#!/usr/bin/env node

import { appendFile, readFile, writeFile } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

export const identityComponents = ['renderer', 'guiNative', 'acpCore', 'terminalCore']

export const declaredComponentActions = {
  renderer: 'restart',
  guiNative: 'restart',
  acpCore: 'restart',
  terminalCore: 'defer-if-active'
}

const canonicalBuildId = /^termul-(renderer|guiNative|acpCore|terminalCore)-v1-sha256:[0-9a-f]{64}$/
const revisionPattern = /^[0-9a-f]{40}$/
const versionDerivedBuildId = /:(?:acp-core|terminal-core|renderer|gui)$/
const updateActions = ['preserve', 'restart', 'defer-if-active', 'unsupported']

function assertPlainObject(value, description) {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${description} must be an object`)
  }
}

function assertNonEmptyString(value, description) {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(`${description} must be a nonempty string`)
  }
}

export function assertCanonicalBuildId(component, buildId) {
  if (typeof buildId !== 'string') {
    throw new Error(`component identity buildId is invalid: ${component}`)
  }
  if (versionDerivedBuildId.test(buildId)) {
    throw new Error(`version-derived component buildId: ${component}`)
  }
  const prefix = `termul-${component}-v1-sha256:`
  if (!canonicalBuildId.test(buildId) || !buildId.startsWith(prefix)) {
    throw new Error(`component identity buildId is invalid: ${component}`)
  }
}

export function assertRevision(revision, description = 'component identity revision') {
  if (typeof revision !== 'string' || !revisionPattern.test(revision)) {
    throw new Error(`${description} must be a 40-character commit SHA`)
  }
}

function assertExactComponents(value, description) {
  assertPlainObject(value, description)
  const actual = Object.keys(value).sort()
  const expected = [...identityComponents].sort()
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${description} must be exactly: ${identityComponents.join(', ')}`)
  }
}

export function assertIdentityArtifact(value) {
  assertPlainObject(value, 'component identity')
  if (value.schemaVersion !== 1) {
    throw new Error('component identity schemaVersion must be 1')
  }
  assertExactComponents(value.components, 'component identity components')
  for (const component of identityComponents) {
    const entry = value.components[component]
    assertPlainObject(entry, `component identity ${component}`)
    assertCanonicalBuildId(component, entry.buildId)
  }
  return value
}

export function assertSameComponentBuildIds(actual, expected, description) {
  assertIdentityArtifact(actual)
  assertIdentityArtifact(expected)
  for (const component of identityComponents) {
    if (actual.components[component].buildId !== expected.components[component].buildId) {
      throw new Error(`${description} component identity drift: ${component}`)
    }
  }
}

export function rustBuildEnv(identity) {
  assertIdentityArtifact(identity)
  return {
    TERMUL_GUI_BUILD_ID: identity.components.guiNative.buildId,
    TERMUL_ACP_CORE_BUILD_ID: identity.components.acpCore.buildId,
    TERMUL_TERMINAL_CORE_BUILD_ID: identity.components.terminalCore.buildId
  }
}

export function bakedIdentityReport(identity, revision) {
  assertIdentityArtifact(identity)
  assertRevision(revision)
  return {
    schemaVersion: 1,
    revision,
    components: Object.fromEntries(
      identityComponents.map((component) => [
        component,
        { buildId: identity.components[component].buildId }
      ])
    ),
    env: rustBuildEnv(identity)
  }
}

export function declaredComponentPolicy(identity, version) {
  assertIdentityArtifact(identity)
  assertNonEmptyString(version, 'version')
  const components = {}
  for (const component of identityComponents) {
    const buildId = identity.components[component].buildId
    if (buildId.startsWith(`${version}:`) || buildId.includes(`:${version}:`)) {
      throw new Error(`version-derived component buildId: ${component}`)
    }
    components[component] = {
      buildId,
      action: declaredComponentActions[component]
    }
  }
  return {
    schemaVersion: 1,
    targetVersion: version,
    metadataState: 'declared',
    components
  }
}

export function assertBakedReport(report, identity, revision) {
  assertPlainObject(report, 'baked component identity')
  if (report.schemaVersion !== 1) {
    throw new Error('baked component identity schemaVersion must be 1')
  }
  assertRevision(report.revision, 'baked component identity revision')
  if (report.revision !== revision) {
    throw new Error(
      `baked component identity revision drift: ${report.revision}, expected ${revision}`
    )
  }
  assertSameComponentBuildIds(report, identity, 'baked')
  const env = rustBuildEnv(identity)
  assertPlainObject(report.env, 'baked component identity env')
  const actualNames = Object.keys(report.env).sort()
  const expectedNames = Object.keys(env).sort()
  if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) {
    throw new Error('baked component identity env drift')
  }
  for (const [name, value] of Object.entries(env)) {
    if (report.env[name] !== value) {
      throw new Error(`baked component identity drift: ${name}`)
    }
  }
}

export function assertManifestMatchesIdentity(manifest, identity, version) {
  assertPlainObject(manifest, 'merged manifest')
  const policy = manifest.termul?.componentPolicy
  if (policy === undefined) throw new Error('merged manifest is missing componentPolicy')
  const expected = declaredComponentPolicy(identity, version)
  assertPlainObject(policy, 'merged manifest componentPolicy')
  if (policy.schemaVersion !== expected.schemaVersion) {
    throw new Error('merged manifest componentPolicy schemaVersion drift')
  }
  if (policy.targetVersion !== expected.targetVersion) {
    throw new Error('merged manifest componentPolicy targetVersion drift')
  }
  if (policy.metadataState !== 'declared') {
    throw new Error('merged manifest componentPolicy metadataState drift')
  }
  assertExactComponents(policy.components, 'merged manifest componentPolicy components')
  for (const component of identityComponents) {
    const entry = policy.components[component]
    const expectedEntry = expected.components[component]
    assertPlainObject(entry, `merged manifest component ${component}`)
    if (entry.buildId !== expectedEntry.buildId) {
      throw new Error(`merged manifest component identity drift: ${component}`)
    }
    if (entry.action !== expectedEntry.action) {
      throw new Error(`merged manifest component action drift: ${component}`)
    }
  }
}

export function retargetPromotedComponentPolicy(policy, { fromVersion, toVersion }) {
  assertNonEmptyString(fromVersion, 'promotion source version')
  assertNonEmptyString(toVersion, 'promotion target version')
  if (fromVersion === toVersion) {
    throw new Error('promotion must retarget a distinct stable version')
  }
  assertPlainObject(policy, 'promotion componentPolicy')
  if (policy.schemaVersion !== 1) {
    throw new Error('promotion componentPolicy schemaVersion must be 1')
  }
  if (policy.metadataState !== 'declared') {
    throw new Error('promotion componentPolicy metadataState must be declared')
  }
  if (policy.targetVersion !== fromVersion) {
    throw new Error(
      `promotion componentPolicy targetVersion drift: ${String(policy.targetVersion)}, expected ${fromVersion}`
    )
  }
  assertExactComponents(policy.components, 'promotion componentPolicy components')
  const components = {}
  for (const component of identityComponents) {
    const entry = policy.components[component]
    assertPlainObject(entry, `promotion componentPolicy ${component}`)
    assertCanonicalBuildId(component, entry.buildId)
    if (entry.buildId.startsWith(`${fromVersion}:`) || entry.buildId.startsWith(`${toVersion}:`)) {
      throw new Error(`version-derived component buildId: ${component}`)
    }
    if (!updateActions.includes(entry.action)) {
      throw new Error(`promotion componentPolicy action is invalid: ${component}`)
    }
    components[component] = { buildId: entry.buildId, action: entry.action }
  }
  return {
    schemaVersion: 1,
    targetVersion: toVersion,
    metadataState: 'declared',
    components
  }
}

async function readJson(path, description) {
  let text
  try {
    text = await readFile(path, 'utf8')
  } catch (error) {
    if (error?.code === 'ENOENT') throw new Error(`Missing ${description}: ${path}`)
    throw error
  }
  try {
    return JSON.parse(text)
  } catch {
    throw new Error(`Malformed ${description}: ${path}`)
  }
}

async function readRevision(path) {
  let text
  try {
    text = await readFile(path, 'utf8')
  } catch (error) {
    if (error?.code === 'ENOENT') throw new Error(`Missing component identity revision: ${path}`)
    throw error
  }
  const revision = text.trim()
  assertRevision(revision)
  return revision
}

function assertRevisionMatch(actual, expected) {
  if (actual !== expected) {
    throw new Error(`component identity revision drift: artifact ${actual}, expected ${expected}`)
  }
}

function componentIdentitySnapshot(components) {
  return JSON.stringify(
    identityComponents.map((component) => ({
      component,
      buildId: components?.[component]?.buildId ?? null,
      action: components?.[component]?.action ?? null
    }))
  )
}

function githubEnvLines(env) {
  return Object.entries(env).map(([key, value]) => {
    if (!/^[A-Z0-9_]+$/.test(key) || /[\r\n]/.test(value)) {
      throw new Error(`refusing to export unsafe component identity env ${key}`)
    }
    return `${key}=${value}`
  })
}

async function writeGithubAssignments(path, env) {
  if (!path) return
  await appendFile(path, `${githubEnvLines(env).join('\n')}\n`)
}

async function writeJson(path, value) {
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`)
}

function parseArgs(argv) {
  const options = { baked: [], manifest: [], requireBaked: false }
  let command
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (!command && !arg.startsWith('--')) {
      command = arg
      continue
    }
    if (arg === '--require-baked') {
      options.requireBaked = true
      continue
    }
    const value = argv[index + 1]
    if (value === undefined || value.startsWith('--')) throw new Error(`Missing value for ${arg}`)
    index += 1
    switch (arg) {
      case '--baked':
        options.baked.push(value)
        break
      case '--manifest':
        options.manifest.push(value)
        break
      case '--identity':
        options.identity = value
        break
      case '--revision-file':
        options.revisionFile = value
        break
      case '--expected-revision':
        options.expectedRevision = value
        break
      case '--github-output':
        options.githubOutput = value
        break
      case '--github-env':
        options.githubEnv = value
        break
      case '--report':
        options.report = value
        break
      case '--version':
        options.version = value
        break
      case '--policy':
        options.policy = value
        break
      case '--regenerated':
        options.regenerated = value
        break
      case '--from-version':
        options.fromVersion = value
        break
      case '--to-version':
        options.toVersion = value
        break
      case '--output':
        options.output = value
        break
      default:
        throw new Error(`Unknown argument: ${arg}`)
    }
  }
  if (!command) {
    throw new Error(
      'Usage: component-identity-release.mjs <bind|inject|policy|check|promote-policy> [options]'
    )
  }
  return { command, options }
}

async function loadIdentity(options) {
  if (!options.identity) throw new Error('Missing component identity path')
  return assertIdentityArtifact(await readJson(options.identity, 'component identity'))
}

async function boundRevision(options, { requiredExpected = false } = {}) {
  if (!options.revisionFile) throw new Error('Missing component identity revision path')
  const revision = await readRevision(options.revisionFile)
  if (requiredExpected && !options.expectedRevision) {
    throw new Error('component identity injection requires a bound revision')
  }
  if (options.expectedRevision) {
    assertRevision(options.expectedRevision, 'expected component identity revision')
    assertRevisionMatch(revision, options.expectedRevision)
  }
  return revision
}

async function commandBind(options) {
  const identity = await loadIdentity(options)
  if (!options.report) throw new Error('Missing baked component identity report path')
  const revision = await boundRevision(options)
  await writeJson(options.report, bakedIdentityReport(identity, revision))
}

async function commandInject(options) {
  const identity = await loadIdentity(options)
  const revision = await boundRevision(options, { requiredExpected: true })
  if (!options.githubOutput && !options.githubEnv) {
    throw new Error('component identity injection requires GitHub output or env')
  }
  const env = rustBuildEnv(identity)
  await writeGithubAssignments(options.githubOutput, env)
  await writeGithubAssignments(options.githubEnv, env)
  if (!options.report) throw new Error('Missing baked component identity report path')
  await writeJson(options.report, bakedIdentityReport(identity, revision))
}

async function commandPolicy(options) {
  const identity = await loadIdentity(options)
  if (!options.version || !options.output) {
    throw new Error('component policy requires --version and --output')
  }
  await writeJson(options.output, declaredComponentPolicy(identity, options.version))
}

async function commandCheck(options) {
  const identity = await loadIdentity(options)
  let revision
  if (options.revisionFile || options.expectedRevision) {
    if (options.revisionFile) revision = await boundRevision(options)
    else {
      assertRevision(options.expectedRevision, 'expected component identity revision')
      revision = options.expectedRevision
    }
  }
  if (options.regenerated) {
    const regenerated = assertIdentityArtifact(
      await readJson(options.regenerated, 'regenerated component identity')
    )
    assertSameComponentBuildIds(regenerated, identity, 'regenerated')
  }
  if (options.requireBaked && options.baked.length === 0) {
    throw new Error('Missing baked component identity')
  }
  if (options.baked.length > 0 && !revision) {
    throw new Error('baked component identity check requires a revision')
  }
  for (const bakedPath of options.baked) {
    assertBakedReport(await readJson(bakedPath, 'baked component identity'), identity, revision)
  }
  if (options.policy) {
    if (!options.version) throw new Error('component policy requires a version')
    await writeJson(options.policy, declaredComponentPolicy(identity, options.version))
  }
  for (const manifestPath of options.manifest) {
    if (!options.version) throw new Error('merged manifest identity check requires a version')
    assertManifestMatchesIdentity(
      await readJson(manifestPath, 'merged manifest'),
      identity,
      options.version
    )
  }
}

async function commandPromote(options) {
  if (options.manifest.length !== 1 || !options.output) {
    throw new Error('promotion requires one --manifest and --output')
  }
  if (!options.fromVersion || !options.toVersion) {
    throw new Error('promotion requires --from-version and --to-version')
  }
  const manifest = await readJson(options.manifest[0], 'promotion manifest')
  assertPlainObject(manifest, 'promotion manifest')
  if (!manifest.termul || manifest.termul.componentPolicy === undefined) {
    throw new Error('promotion is missing componentPolicy')
  }
  const before = componentIdentitySnapshot(manifest.termul.componentPolicy.components)
  manifest.termul.componentPolicy = retargetPromotedComponentPolicy(
    manifest.termul.componentPolicy,
    {
      fromVersion: options.fromVersion,
      toVersion: options.toVersion
    }
  )
  const after = componentIdentitySnapshot(manifest.termul.componentPolicy.components)
  if (after !== before) throw new Error('promotion changed componentPolicy identities')
  await writeJson(options.output, manifest)
}

async function runCli(argv) {
  const { command, options } = parseArgs(argv)
  switch (command) {
    case 'bind':
      await commandBind(options)
      return
    case 'inject':
      await commandInject(options)
      return
    case 'policy':
      await commandPolicy(options)
      return
    case 'check':
      await commandCheck(options)
      return
    case 'promote-policy':
      await commandPromote(options)
      return
    default:
      throw new Error(`Unknown command: ${command}`)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runCli(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  })
}
