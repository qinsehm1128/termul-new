import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { describe, expect, test } from 'vitest'
import { parse } from 'yaml'
// @ts-expect-error The production helper is intentionally plain ESM for direct workflow use.
import { generateComponentIdentities } from './component-build-id.mjs'
import * as identityRelease from './component-identity-release.mjs'
// @ts-expect-error The production helper is intentionally plain ESM for direct workflow use.
import { mergeUpdaterManifests, requiredPlatformKeys } from './merge-updater-manifests.mjs'

type ComponentPolicy = {
  schemaVersion: number
  targetVersion: string
  metadataState: string
  components: Record<string, { buildId: string; action: string }>
  pluginCore?: { buildId: string; action: string }
}

type Identity = {
  schemaVersion: number
  components: Record<string, { buildId: string }>
}

type ReleaseHelper = {
  assertManifestMatchesIdentity: (manifest: unknown, identity: Identity, version: string) => void
  bakedIdentityReport: (identity: Identity, revision: string) => unknown
  declaredComponentActions: Record<string, string>
  declaredComponentPolicy: (identity: Identity, version: string) => ComponentPolicy
  identityComponents: string[]
  retargetPromotedComponentPolicy: (
    policy: ComponentPolicy,
    versions: { fromVersion: string; toVersion: string }
  ) => ComponentPolicy
  rustBuildEnv: (identity: Identity) => Record<string, string>
  normalizeComponentPolicy: (
    policy: ComponentPolicy,
    version: string
  ) => ComponentPolicy['components']
  publicComponentPolicy: (policy: ComponentPolicy) => ComponentPolicy
}

const {
  assertManifestMatchesIdentity,
  bakedIdentityReport,
  declaredComponentActions,
  declaredComponentPolicy,
  identityComponents,
  retargetPromotedComponentPolicy,
  rustBuildEnv,
  normalizeComponentPolicy,
  publicComponentPolicy
} = identityRelease as unknown as ReleaseHelper
const execFileAsync = promisify(execFile)
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const scriptPath = join(repoRoot, 'scripts/release/component-identity-release.mjs')
const releaseVersion = '1.2.3'
const revision = 'a'.repeat(40)
const otherRevision = 'b'.repeat(40)

function canonicalBuildId(component: string, seed: string) {
  const digest = createHash('sha256').update(seed).digest('hex')
  return `termul-${component}-v1-sha256:${digest}`
}

function identityFor(seed = 'seed') {
  return {
    schemaVersion: 1,
    components: Object.fromEntries(
      identityComponents.map((component: string) => [
        component,
        { buildId: canonicalBuildId(component, `${seed}:${component}`) }
      ])
    )
  }
}

async function fixtureDir() {
  return mkdtemp(join(tmpdir(), 'se-component-identity-release-'))
}

async function runCli(args: string[], cwd = repoRoot) {
  try {
    return await execFileAsync(process.execPath, [scriptPath, ...args], { cwd })
  } catch (error) {
    const failure = error as { stderr?: string; stdout?: string; message?: string }
    throw new Error(failure.stderr || failure.stdout || failure.message)
  }
}

function updaterRecord(key: string) {
  const assetName = `Se-${releaseVersion}-${key}.bin`
  return {
    assetName,
    record: {
      url: `https://github.com/qinsehm1128/termul-new/releases/download/v${releaseVersion}/${assetName}`,
      signature: `signature-${key}`
    }
  }
}

async function writeCompleteManifest(dir: string) {
  const platforms: Record<string, unknown> = {}
  const assetNames: string[] = []
  for (const key of requiredPlatformKeys as string[]) {
    const entry = updaterRecord(key)
    platforms[key] = entry.record
    assetNames.push(entry.assetName, `${entry.assetName}.sig`)
  }
  const path = join(dir, 'platform.json')
  await writeFile(path, JSON.stringify({ version: releaseVersion, assetNames, platforms }))
  return path
}

function extractJob(workflow: string, name: string) {
  const match = workflow.match(new RegExp(`^  ${name}:$`, 'm'))
  if (!match || match.index === undefined) throw new Error(`missing job ${name}`)
  const start = match.index
  const rest = workflow.slice(start + 1)
  const next = rest.search(/^ {2}[A-Za-z0-9_-]+:$/m)
  return workflow.slice(start, next < 0 ? workflow.length : start + 1 + next)
}

describe('component identity release propagation', () => {
  test('builds declared policy and rust env from one canonical artifact', () => {
    const identity = identityFor()
    const policy = declaredComponentPolicy(identity, releaseVersion)

    expect(policy).toEqual({
      schemaVersion: 1,
      targetVersion: releaseVersion,
      metadataState: 'declared',
      components: {
        renderer: { buildId: identity.components.renderer.buildId, action: 'restart' },
        guiNative: { buildId: identity.components.guiNative.buildId, action: 'restart' },
        acpCore: { buildId: identity.components.acpCore.buildId, action: 'defer-if-active' },
        pluginCore: { buildId: identity.components.pluginCore.buildId, action: 'defer-if-active' },
        terminalCore: {
          buildId: identity.components.terminalCore.buildId,
          action: 'defer-if-active'
        }
      }
    })
    expect(policy.components.acpCore.buildId).not.toContain(`${releaseVersion}:`)
    expect(policy.components.terminalCore.buildId).not.toContain(`${releaseVersion}:`)
    expect(rustBuildEnv(identity)).toEqual({
      TERMUL_GUI_BUILD_ID: identity.components.guiNative.buildId,
      TERMUL_ACP_CORE_BUILD_ID: identity.components.acpCore.buildId,
      TERMUL_TERMINAL_CORE_BUILD_ID: identity.components.terminalCore.buildId,
      TERMUL_PLUGIN_CORE_BUILD_ID: identity.components.pluginCore.buildId
    })
    expect(rustBuildEnv(identity)).not.toHaveProperty('TERMUL_RENDERER_BUILD_ID')
  })

  test('keeps Core IDs stable when only the app version changes', async () => {
    const root = await mkdtemp(join(tmpdir(), 'se-component-identity-policy-'))
    const inputs = {
      schemaVersion: 1,
      sharedFiles: ['src-tauri/Cargo.toml'],
      components: {
        renderer: ['package.json'],
        guiNative: [],
        pluginCore: [],
        acpCore: ['src-tauri/src/acp.rs'],
        terminalCore: ['src-tauri/src/pty.rs']
      }
    }
    try {
      await mkdir(join(root, 'src-tauri/src'), { recursive: true })
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        '[package]\nname = "se-manager"\nversion = "0.12.6"\n'
      )
      await writeFile(join(root, 'package.json'), '{"version":"0.12.6"}\n')
      await writeFile(join(root, 'src-tauri/src/acp.rs'), 'pub const ACP: u8 = 1;\n')
      await writeFile(join(root, 'src-tauri/src/pty.rs'), 'pub const PTY: u8 = 1;\n')
      const first = await generateComponentIdentities({ root, inputs })
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        '[package]\nname = "se-manager"\nversion = "0.12.7"\n'
      )
      const second = await generateComponentIdentities({ root, inputs })
      const policy = declaredComponentPolicy(first, '0.12.7')

      expect(second.components.acpCore.buildId).toBe(first.components.acpCore.buildId)
      expect(second.components.terminalCore.buildId).toBe(first.components.terminalCore.buildId)
      expect(policy.components.acpCore.buildId).toBe(first.components.acpCore.buildId)
      expect(policy.components.terminalCore.buildId).toBe(first.components.terminalCore.buildId)
      expect(policy.components.acpCore.action).toBe(declaredComponentActions.acpCore)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('merged manifest IDs equal the generator artifact', async () => {
    const dir = await fixtureDir()
    try {
      const identity = identityFor('merge')
      const policy = declaredComponentPolicy(identity, releaseVersion)
      const merged = await mergeUpdaterManifests({
        inputPaths: [await writeCompleteManifest(dir)],
        outputPath: join(dir, 'latest.json'),
        version: releaseVersion,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z',
        componentPolicy: policy
      })

      assertManifestMatchesIdentity(merged, identity, releaseVersion)
      expect(
        JSON.parse(await readFile(join(dir, 'latest.json'), 'utf8')).termul.componentPolicy
      ).toEqual(publicComponentPolicy(policy))
    } finally {
      await rm(dir, { recursive: true })
    }
  })

  test('release check fails closed on missing or drifted baked identity', async () => {
    const dir = await fixtureDir()
    try {
      const identity = identityFor('check')
      const identityPath = join(dir, 'component-identities.json')
      const revisionPath = join(dir, 'revision.txt')
      const reportPath = join(dir, 'baked-component-identity.json')
      const driftedPath = join(dir, 'drifted.json')
      const policyPath = join(dir, 'component-policy.json')
      await writeFile(identityPath, JSON.stringify(identity))
      await writeFile(revisionPath, `${revision}\n`)
      await runCli([
        'bind',
        '--identity',
        identityPath,
        '--revision-file',
        revisionPath,
        '--report',
        reportPath
      ])
      await writeFile(
        driftedPath,
        JSON.stringify(bakedIdentityReport(identityFor('other'), revision))
      )

      await expect(
        runCli([
          'check',
          '--identity',
          identityPath,
          '--revision-file',
          revisionPath,
          '--expected-revision',
          revision,
          '--require-baked',
          '--version',
          releaseVersion,
          '--policy',
          policyPath
        ])
      ).rejects.toThrow('Missing baked component identity')

      await expect(
        runCli([
          'check',
          '--identity',
          identityPath,
          '--revision-file',
          revisionPath,
          '--expected-revision',
          otherRevision,
          '--baked',
          reportPath
        ])
      ).rejects.toThrow('component identity revision drift')

      await expect(
        runCli([
          'check',
          '--identity',
          identityPath,
          '--revision-file',
          revisionPath,
          '--expected-revision',
          revision,
          '--regenerated',
          join(dir, 'missing.json'),
          '--baked',
          reportPath
        ])
      ).rejects.toThrow('Missing regenerated component identity')

      await writeFile(join(dir, 'regenerated.json'), JSON.stringify(identityFor('other')))
      await expect(
        runCli([
          'check',
          '--identity',
          identityPath,
          '--revision-file',
          revisionPath,
          '--regenerated',
          join(dir, 'regenerated.json'),
          '--baked',
          reportPath
        ])
      ).rejects.toThrow('regenerated component identity drift')

      await expect(
        runCli([
          'check',
          '--identity',
          identityPath,
          '--revision-file',
          revisionPath,
          '--expected-revision',
          revision,
          '--baked',
          driftedPath,
          '--baked',
          reportPath
        ])
      ).rejects.toThrow('baked component identity drift')

      const manifest = join(dir, 'latest.json')
      await writeFile(
        manifest,
        JSON.stringify({
          version: releaseVersion,
          termul: { componentPolicy: declaredComponentPolicy(identityFor('other'), releaseVersion) }
        })
      )
      await expect(
        runCli([
          'check',
          '--identity',
          identityPath,
          '--version',
          releaseVersion,
          '--manifest',
          manifest
        ])
      ).rejects.toThrow('merged manifest component identity drift')

      await runCli([
        'policy',
        '--identity',
        identityPath,
        '--version',
        releaseVersion,
        '--output',
        policyPath
      ])
      const policy = JSON.parse(await readFile(policyPath, 'utf8'))
      await writeFile(
        manifest,
        JSON.stringify({ version: releaseVersion, termul: { componentPolicy: policy } })
      )
      await runCli([
        'check',
        '--identity',
        identityPath,
        '--revision-file',
        revisionPath,
        '--expected-revision',
        revision,
        '--regenerated',
        identityPath,
        '--baked',
        reportPath,
        '--require-baked',
        '--version',
        releaseVersion,
        '--manifest',
        manifest
      ])
      expect(policy.components.terminalCore.buildId).toBe(identity.components.terminalCore.buildId)
    } finally {
      await rm(dir, { recursive: true })
    }
  })

  test('promotion retargets the version without changing component identities', async () => {
    const dir = await fixtureDir()
    try {
      const identity = identityFor('promote')
      const fromVersion = '1.2.3-rc.1'
      const toVersion = '1.2.3'
      const policy = declaredComponentPolicy(identity, fromVersion)
      policy.components.acpCore.action = 'restart'
      const manifest = {
        version: fromVersion,
        notes: 'rc',
        termul: { componentPolicy: policy },
        platforms: { 'darwin-aarch64': { url: 'https://example.invalid', signature: 'sig' } }
      }
      const input = join(dir, 'latest-insider.json')
      const output = join(dir, 'latest-stable.json')
      await writeFile(input, JSON.stringify(manifest))

      await runCli([
        'promote-policy',
        '--manifest',
        input,
        '--from-version',
        fromVersion,
        '--to-version',
        toVersion,
        '--output',
        output
      ])
      const promoted = JSON.parse(await readFile(output, 'utf8'))

      expect(promoted.termul.componentPolicy.targetVersion).toBe(toVersion)
      expect(normalizeComponentPolicy(promoted.termul.componentPolicy, toVersion)).toEqual(
        policy.components
      )
      expect(promoted.platforms).toEqual(manifest.platforms)
      expect(
        normalizeComponentPolicy(
          retargetPromotedComponentPolicy(policy, { fromVersion, toVersion }),
          toVersion
        )
      ).toEqual(policy.components)
      await expect(
        runCli([
          'promote-policy',
          '--manifest',
          input,
          '--from-version',
          toVersion,
          '--to-version',
          fromVersion,
          '--output',
          output
        ])
      ).rejects.toThrow('promotion componentPolicy targetVersion drift')

      const derived = structuredClone(policy)
      derived.components.terminalCore.buildId = `${fromVersion}:terminal-core`
      expect(() => retargetPromotedComponentPolicy(derived, { fromVersion, toVersion })).toThrow(
        'version-derived component buildId: terminalCore'
      )
    } finally {
      await rm(dir, { recursive: true })
    }
  })

  test('release and nightly workflows propagate one revision-bound identity', async () => {
    const workflowDir = join(repoRoot, '.github/workflows')
    const release = await readFile(join(workflowDir, 'release.yml'), 'utf8')
    const nightly = await readFile(join(workflowDir, 'nightly.yml'), 'utf8')

    for (const workflow of [release, nightly]) {
      expect(() => parse(workflow)).not.toThrow()
      expect(workflow).not.toMatch(/\$\{VERSION\}:(?:acp-core|terminal-core|renderer|gui)\b/)
      expect(workflow).not.toContain(':acp-core')
      expect(workflow).not.toContain(':terminal-core')
      expect(workflow).toContain('scripts/release/component-build-id.mjs')
      expect(workflow).toContain('scripts/release/component-identity-release.mjs')
      expect(workflow).toContain('touch src-tauri/crates/se-terminal-core/src/ipc.rs')
      const desktop = workflow.slice(
        workflow.indexOf('name: Build platform artifacts locally'),
        workflow.indexOf('name: Persist Tauri artifact paths')
      )
      expect(desktop).toContain('TERMUL_GUI_BUILD_ID:')
      expect(desktop).toContain('TERMUL_ACP_CORE_BUILD_ID:')
      expect(desktop).toContain('TERMUL_TERMINAL_CORE_BUILD_ID:')
      expect(desktop).toContain('steps.component-identity.outputs.TERMUL_GUI_BUILD_ID')
      const build = extractJob(workflow, 'build')
      expect(build).toContain('component-identity-release.mjs inject')
      expect(build).not.toContain('component-build-id.mjs')
      expect(build).toContain('baked-component-identity.json')
    }

    const releaseIdentity = extractJob(release, 'component_identity')
    expect(releaseIdentity).toContain('component-build-id.mjs')
    expect(releaseIdentity).not.toContain('align-version.ts')
    expect(releaseIdentity).toContain('release-component-identity')
    const releaseBuild = extractJob(release, 'build')
    expect(releaseBuild).toContain('needs.component_identity.outputs.commit_sha')
    expect(releaseBuild).toContain('release-component-identity')
    const releasePublish = extractJob(release, 'publish_release')
    expect(releasePublish).toContain('component-build-id.mjs')
    expect(releasePublish).not.toContain('align-version.ts')
    expect(releasePublish).toContain('--require-baked')
    expect(releasePublish).toContain('release-component-identity')
    expect(releasePublish).toContain('needs.component_identity.outputs.commit_sha')
    const promote = extractJob(release, 'promote')
    expect(promote).toContain('promote-policy')
    expect(promote).toContain('componentPolicy identities changed')

    const prepare = extractJob(nightly, 'prepare')
    expect(prepare).toContain('component-build-id.mjs')
    expect(prepare).not.toContain('align-version.ts')
    expect(prepare).toContain('nightly-component-identity')
    const nightlyBuild = extractJob(nightly, 'build')
    expect(nightlyBuild).toContain('needs.prepare.outputs.commit_sha')
    expect(nightlyBuild).toContain('nightly-component-identity')
    const nightlyPublish = extractJob(nightly, 'publish')
    expect(nightlyPublish).toContain('component-build-id.mjs')
    expect(nightlyPublish).not.toContain('align-version.ts')
    expect(nightlyPublish).toContain('--require-baked')
    expect(nightlyPublish).toContain('nightly-component-identity')
    expect(nightlyPublish).toContain('needs.prepare.outputs.commit_sha')
  })
})

// Frozen verbatim from HEAD updater.types.ts before the Plugin Core change.
// Do NOT import the evolving shared parser: this proves shipped clients can parse new manifests.
const frozenFourKeyParser = (() => {
  const UPDATE_COMPONENTS = ['renderer', 'guiNative', 'acpCore', 'terminalCore'] as const

  type UpdateComponent = (typeof UPDATE_COMPONENTS)[number]

  type UpdateComponentAction = 'preserve' | 'restart' | 'defer-if-active' | 'unsupported'

  interface UpdateComponentPolicyEntry {
    buildId: string
    action: UpdateComponentAction
  }

  interface UpdateComponentPolicy {
    schemaVersion: 1
    targetVersion: string
    metadataState: 'declared' | 'legacy'
    components: Record<UpdateComponent, UpdateComponentPolicyEntry>
  }

  function legacyUpdateComponentPolicy(version: string): UpdateComponentPolicy {
    const components = Object.fromEntries(
      UPDATE_COMPONENTS.map((component) => [
        component,
        {
          buildId: `legacy:${version}:${component}`,
          action: component === 'terminalCore' ? 'defer-if-active' : 'restart'
        }
      ])
    ) as Record<UpdateComponent, UpdateComponentPolicyEntry>

    return {
      schemaVersion: 1,
      targetVersion: version,
      metadataState: 'legacy',
      components
    }
  }

  function parseUpdateComponentPolicy(value: unknown, version: string): UpdateComponentPolicy {
    if (value === undefined) return legacyUpdateComponentPolicy(version)
    if (typeof value !== 'object' || value === null || Array.isArray(value)) {
      throw new Error('Update component policy must be an object')
    }

    const candidate = value as {
      schemaVersion?: unknown
      targetVersion?: unknown
      metadataState?: unknown
      components?: unknown
    }
    if (candidate.schemaVersion !== 1) {
      throw new Error('Update component policy schemaVersion must be 1')
    }
    if (candidate.targetVersion !== version) {
      throw new Error('Update component policy targetVersion does not match the update version')
    }
    if (candidate.metadataState !== 'declared' && candidate.metadataState !== 'legacy') {
      throw new Error('Update component policy metadataState is invalid')
    }
    if (
      typeof candidate.components !== 'object' ||
      candidate.components === null ||
      Array.isArray(candidate.components)
    ) {
      throw new Error('Update component policy components must be an object')
    }

    const componentRecord = candidate.components as Record<string, unknown>
    const keys = Object.keys(componentRecord).sort()
    const expectedKeys = [...UPDATE_COMPONENTS].sort()
    if (
      keys.length !== expectedKeys.length ||
      keys.some((key, index) => key !== expectedKeys[index])
    ) {
      throw new Error(
        'Update component policy components are incomplete or contain unknown entries'
      )
    }

    const components = {} as Record<UpdateComponent, UpdateComponentPolicyEntry>
    for (const component of UPDATE_COMPONENTS) {
      const entry = componentRecord[component]
      if (typeof entry !== 'object' || entry === null || Array.isArray(entry)) {
        throw new Error(`Update component policy entry is invalid: ${component}`)
      }
      const candidateEntry = entry as { buildId?: unknown; action?: unknown }
      if (typeof candidateEntry.buildId !== 'string' || candidateEntry.buildId.trim() === '') {
        throw new Error(`Update component policy buildId is invalid: ${component}`)
      }
      if (
        candidateEntry.action !== 'preserve' &&
        candidateEntry.action !== 'restart' &&
        candidateEntry.action !== 'defer-if-active' &&
        candidateEntry.action !== 'unsupported'
      ) {
        throw new Error(`Update component policy action is invalid: ${component}`)
      }
      components[component] = {
        buildId: candidateEntry.buildId.trim(),
        action: candidateEntry.action
      }
    }

    return {
      schemaVersion: 1,
      targetVersion: version,
      metadataState: candidate.metadataState,
      components
    }
  }

  return parseUpdateComponentPolicy
})()

describe('Plugin Core public wire compatibility', () => {
  test('frozen current four-key parser accepts new manifests and promotion output', () => {
    const identity = identityFor('wire')
    const policy = publicComponentPolicy(declaredComponentPolicy(identity, '1.0.0'))
    expect(Object.keys(policy.components).sort()).toEqual(
      ['renderer', 'guiNative', 'acpCore', 'terminalCore'].sort()
    )
    expect(policy.pluginCore).toEqual({
      buildId: identity.components.pluginCore.buildId,
      action: 'defer-if-active'
    })
    expect(frozenFourKeyParser(policy, '1.0.0').components).toEqual(policy.components)
    const promoted = retargetPromotedComponentPolicy(policy, {
      fromVersion: '1.0.0',
      toVersion: '1.0.1'
    })
    expect(frozenFourKeyParser(promoted, '1.0.1').components).toEqual(policy.components)
    expect(promoted.pluginCore).toEqual(policy.pluginCore)
    expect(normalizeComponentPolicy(policy, '1.0.0')).toEqual(
      declaredComponentPolicy(identity, '1.0.0').components
    )
  })

  test('legacy four-key policies synthesize unsupported plugin metadata', () => {
    const policy = publicComponentPolicy(declaredComponentPolicy(identityFor('old'), '1.0.0'))
    delete policy.pluginCore
    expect(normalizeComponentPolicy(policy, '1.0.0').pluginCore).toEqual({
      buildId: 'legacy:1.0.0:pluginCore',
      action: 'unsupported'
    })
  })

  test('normalizes matching duplicates and rejects conflicts or unknown components', () => {
    const internal = declaredComponentPolicy(identityFor('duplicate'), '1.0.0')
    const duplicate = { ...internal, pluginCore: { ...internal.components.pluginCore } }
    expect(normalizeComponentPolicy(duplicate, '1.0.0')).toEqual(internal.components)
    duplicate.pluginCore.action = 'restart'
    expect(() => normalizeComponentPolicy(duplicate, '1.0.0')).toThrow(
      /conflicting duplicate pluginCore/
    )
    const extra = {
      ...internal,
      components: { ...internal.components, unknown: internal.components.pluginCore }
    }
    expect(() => normalizeComponentPolicy(extra, '1.0.0')).toThrow(/exactly/)
  })

  test('canonical Plugin Core IDs are exact 64-hex namespace-bound identities', () => {
    const identity = identityFor('canonical-plugin')
    expect(identity.components.pluginCore.buildId).toMatch(
      /^termul-pluginCore-v1-sha256:[0-9a-f]{64}$/
    )
    for (const buildId of [
      'plugin-v1',
      canonicalBuildId('terminalCore', 'wrong'),
      'termul-pluginCore-v1-sha256:abc'
    ]) {
      const bad = structuredClone(identity)
      bad.components.pluginCore.buildId = buildId
      expect(() => rustBuildEnv(bad)).toThrow(/buildId.*pluginCore/)
    }
  })
})
