#!/usr/bin/env node

import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative, resolve, sep } from 'node:path'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

const DEFAULT_INPUTS = new URL('./component-build-inputs.json', import.meta.url)
const ID_SCHEMA_VERSION = 1
const COMPONENTS = ['renderer', 'guiNative', 'acpCore', 'terminalCore']

function assertPlainObject(value, description) {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${description} must be an object`)
  }
}

function normalizeText(path, content) {
  const normalized = content.replaceAll('\r\n', '\n').replaceAll('\r', '\n')
  if (path !== 'src-tauri/Cargo.toml' && path !== 'src-tauri/Cargo.lock') return normalized

  return normalized
    .replace(/^version = "[^"]+"$/m, 'version = "<package-version>"')
    .replace(/(name = "se-manager"\nversion = )"[^"]+"/g, '$1"<package-version>"')
}

function isInside(root, candidate) {
  const rootRelative = relative(root, candidate)
  return rootRelative === '' || (rootRelative !== '..' && !rootRelative.startsWith(`..${sep}`))
}

async function expandInput(root, input) {
  const absolute = resolve(root, input)
  if (!isInside(root, absolute)) {
    throw new Error(`Input path escapes repository root: ${input}`)
  }

  let stat
  try {
    stat = await readdir(absolute, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOTDIR') return [input]
    throw new Error(`Missing component identity input: ${input}`)
  }

  const files = []
  for (const entry of stat.sort((left, right) => left.name.localeCompare(right.name))) {
    const child = `${input}/${entry.name}`
    if (entry.isDirectory()) files.push(...(await expandInput(root, child)))
    else if (entry.isFile()) files.push(child)
  }
  return files
}

async function readInputs(root, inputs) {
  assertPlainObject(inputs, 'component identity inputs')
  if (inputs.schemaVersion !== ID_SCHEMA_VERSION) {
    throw new Error(`component identity input schemaVersion must be ${ID_SCHEMA_VERSION}`)
  }
  if (
    !Array.isArray(inputs.sharedFiles) ||
    !inputs.sharedFiles.every((value) => typeof value === 'string')
  ) {
    throw new Error('component identity sharedFiles must be an array of strings')
  }
  assertPlainObject(inputs.components, 'component identity components')
  if (inputs.cargo !== undefined) {
    assertPlainObject(inputs.cargo, 'component identity cargo')
    for (const key of ['manifest', 'lock', 'package', 'crateSource', 'workspaceCrates']) {
      if (typeof inputs.cargo[key] !== 'string') {
        throw new Error(`component identity cargo.${key} must be a string`)
      }
    }
    const scoped = inputs.cargo.scopedComponents
    if (!Array.isArray(scoped) || !scoped.every((value) => COMPONENTS.includes(value))) {
      throw new Error('component identity cargo.scopedComponents must name components')
    }
    const crates = inputs.cargo.crateComponents ?? {}
    assertPlainObject(crates, 'component identity cargo.crateComponents')
    for (const [component, name] of Object.entries(crates)) {
      if (!COMPONENTS.includes(component) || typeof name !== 'string') {
        throw new Error('component identity cargo.crateComponents must map components to crates')
      }
      if (scoped.includes(component)) {
        throw new Error(`component identity ${component} cannot be both scoped and crate-built`)
      }
    }
  }

  const expanded = new Map()
  const add = async (path) => {
    for (const file of await expandInput(root, path)) expanded.set(file, true)
  }

  const crateMaterial = {}
  for (const [component, name] of Object.entries(inputs.cargo?.crateComponents ?? {})) {
    crateMaterial[component] = await crateComponentMaterial(root, inputs.cargo, name)
    for (const dir of crateMaterial[component].crateDirs) await add(dir)
  }

  for (const path of inputs.sharedFiles) await add(path)
  for (const component of COMPONENTS) {
    const paths = inputs.components[component]
    if (!Array.isArray(paths) || !paths.every((value) => typeof value === 'string')) {
      throw new Error(`component identity inputs missing ${component}`)
    }
    for (const path of paths) await add(path)
  }

  const fileDigests = new Map()
  for (const path of [...expanded.keys()].sort()) {
    const content = await readFile(resolve(root, path))
    const text = normalizeText(path, content.toString('utf8'))
    const digest = createHash('sha256').update(text, 'utf8').digest('hex')
    fileDigests.set(path, digest)
  }

  return { fileDigests, crateMaterial }
}

const CARGO_PACKAGE_VERSION = 'version = "<package-version>"'

/** Drop a TOML `#` comment that is not inside a string. */
function stripTomlComment(line) {
  let quoted = false
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index]
    if (char === '"' && line[index - 1] !== '\\') quoted = !quoted
    else if (char === '#' && !quoted) return line.slice(0, index)
  }
  return line
}

function bracketDepth(text) {
  let depth = 0
  let quoted = false
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index]
    if (char === '"' && text[index - 1] !== '\\') quoted = !quoted
    else if (!quoted && (char === '[' || char === '{')) depth += 1
    else if (!quoted && (char === ']' || char === '}')) depth -= 1
  }
  return depth
}

/**
 * Split a Cargo manifest into the part every Core shares (package, features,
 * profiles, build dependencies, ...) and its runtime dependency entries.
 * Comments and the package version are dropped: neither changes a binary.
 * Dev-dependencies are dropped too; they never reach a release build.
 */
export function parseCargoManifest(text) {
  const shared = []
  const dependencies = []
  let table = ''
  let pending = null
  const lines = text.replaceAll('\r\n', '\n').split('\n')
  for (const raw of lines) {
    const line = stripTomlComment(raw).trimEnd()
    if (pending) {
      pending.text += ` ${line.trim()}`
      if (bracketDepth(pending.text) <= 0) {
        dependencies.push(pending)
        pending = null
      }
      continue
    }
    if (line.trim() === '') continue
    const header = /^\s*\[{1,2}([^\]]+)\]{1,2}\s*$/.exec(line)
    if (header) {
      table = header[1].trim()
      if (!isDependencyTable(table)) shared.push(line.trim())
      continue
    }
    if (isDevDependencyTable(table)) continue
    if (isDependencyTable(table)) {
      const entry = /^\s*([A-Za-z0-9_-]+)\s*=/.exec(line)
      if (!entry) continue
      const item = { table, name: entry[1], text: line.trim() }
      if (bracketDepth(item.text) > 0) pending = item
      else dependencies.push(item)
      continue
    }
    shared.push(
      table === 'package' && /^version\s*=/.test(line.trim()) ? CARGO_PACKAGE_VERSION : line.trim()
    )
  }
  return { shared, dependencies }
}

function isDevDependencyTable(table) {
  return /(^|\.)dev-dependencies$/.test(table)
}

function isDependencyTable(table) {
  return /(^|\.)(dev-)?dependencies$/.test(table)
}

/** `[[package]]` entries of a Cargo.lock, keyed by name. */
export function parseCargoLock(text) {
  const packages = new Map()
  for (const block of text.replaceAll('\r\n', '\n').split('\n[[package]]\n').slice(1)) {
    const field = (name) => new RegExp(`^${name} = "([^"]*)"$`, 'm').exec(block)?.[1]
    const list = /^dependencies = \[\n([\s\S]*?)\n\]$/m.exec(block)?.[1] ?? ''
    const entry = {
      name: field('name'),
      version: field('version'),
      source: field('source') ?? '',
      checksum: field('checksum') ?? '',
      dependencies: [...list.matchAll(/^ "([^"]+)",?$/gm)].map((match) => match[1])
    }
    packages.set(entry.name, [...(packages.get(entry.name) ?? []), entry])
  }
  return packages
}

/** A lock dependency spec is `name`, `name version` or `name version (source)`. */
function resolveLockSpec(packages, spec) {
  const [name, version] = spec.split(' ')
  const candidates = packages.get(name) ?? []
  const match = version ? candidates.find((entry) => entry.version === version) : candidates[0]
  if (!match) throw new Error(`Cargo.lock has no package for dependency "${spec}"`)
  return match
}

/** Every lock package reachable from `roots`, keyed `name version`. */
function lockClosureEntries(packages, rootSpecs) {
  const seen = new Map()
  const queue = rootSpecs.map((spec) => resolveLockSpec(packages, spec))
  while (queue.length > 0) {
    const entry = queue.pop()
    const key = `${entry.name} ${entry.version}`
    if (seen.has(key)) continue
    seen.set(key, entry)
    for (const spec of entry.dependencies) queue.push(resolveLockSpec(packages, spec))
  }
  return seen
}

/** Every lock package reachable from `roots`, as stable text lines. */
function lockClosure(packages, rootSpecs) {
  return [...lockClosureEntries(packages, rootSpecs).entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(
      ([key, entry]) =>
        `${key} ${entry.source} ${entry.checksum} [${[...entry.dependencies].sort().join(', ')}]`
    )
}

/**
 * Cargo material a scoped Core's identity hashes: the shared manifest part,
 * the manifest entries of the dependencies the Core's own code names (plus
 * the workspace crates it lists), and the Cargo.lock closure under them. A
 * dependency only the GUI uses then leaves the Core's identity alone.
 */
async function scopedCargoMaterial(root, cargo, componentPaths) {
  const manifest = parseCargoManifest(await readFile(resolve(root, cargo.manifest), 'utf8'))
  const packages = parseCargoLock(await readFile(resolve(root, cargo.lock), 'utf8'))

  const sources = []
  const workspaceCrates = new Set()
  for (const path of componentPaths) {
    for (const file of await expandInput(root, path)) {
      if (file.startsWith(`${cargo.crateSource}/`) && file.endsWith('.rs')) {
        sources.push(await readFile(resolve(root, file), 'utf8'))
      }
      const crate = new RegExp(`^${cargo.workspaceCrates}/([^/]+)/Cargo\\.toml$`).exec(file)
      if (crate) {
        const toml = await readFile(resolve(root, file), 'utf8')
        const name = /^name = "([^"]+)"$/m.exec(toml)?.[1]
        if (name) workspaceCrates.add(name)
      }
    }
  }
  const code = sources.join('\n')
  const roots = new Set(workspaceCrates)
  for (const { name } of manifest.dependencies) {
    const ident = name.replaceAll('-', '_')
    if (new RegExp(`\\b${ident}::|\\buse ${ident}\\b`).test(code)) roots.add(name)
  }

  const own = (packages.get(cargo.package) ?? [])[0]
  if (!own) throw new Error(`Cargo.lock has no package ${cargo.package}`)
  const rootSpecs = own.dependencies.filter((spec) => roots.has(spec.split(' ')[0]))

  const entries = manifest.dependencies
    .filter((entry) => roots.has(entry.name))
    .map((entry) => `[${entry.table}] ${entry.text}`)
    .sort()
  const digest = (lines) => createHash('sha256').update(lines.join('\n'), 'utf8').digest('hex')
  return {
    roots: [...roots].sort(),
    manifestSha256: digest([...manifest.shared, ...entries]),
    lockSha256: digest(lockClosure(packages, rootSpecs))
  }
}

/** Workspace crate directories under `cargo.workspaceCrates`, by package name. */
async function workspaceCrateDirs(root, cargo) {
  const dirs = new Map()
  const entries = await readdir(resolve(root, cargo.workspaceCrates), { withFileTypes: true })
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    if (!entry.isDirectory()) continue
    const dir = `${cargo.workspaceCrates}/${entry.name}`
    const toml = await readFile(resolve(root, dir, 'Cargo.toml'), 'utf8').catch(() => null)
    const name = toml && /^name = "([^"]+)"$/m.exec(toml)?.[1]
    if (name) dirs.set(name, dir)
  }
  return dirs
}

/**
 * Material for a Core whose executable links a single workspace crate, so
 * Cargo.lock names everything it runs: the workspace crates in that crate's
 * dependency closure (their directories are hashed as files), the shared
 * manifest part, the root manifest's entries for packages in the closure
 * (their features unify with the app's in one build), and the closure.
 */
async function crateComponentMaterial(root, cargo, crateName) {
  const manifest = parseCargoManifest(await readFile(resolve(root, cargo.manifest), 'utf8'))
  const packages = parseCargoLock(await readFile(resolve(root, cargo.lock), 'utf8'))
  const closure = lockClosureEntries(packages, [crateName])
  const names = new Set([...closure.values()].map((entry) => entry.name))
  const dirs = await workspaceCrateDirs(root, cargo)
  const crateDirs = [...names]
    .filter((name) => dirs.has(name))
    .map((name) => dirs.get(name))
    .sort()
  const entries = manifest.dependencies
    .filter((entry) => names.has(entry.name))
    .map((entry) => `[${entry.table}] ${entry.text}`)
    .sort()
  const digest = (lines) => createHash('sha256').update(lines.join('\n'), 'utf8').digest('hex')
  return {
    crateDirs,
    cargo: {
      crate: crateName,
      manifestSha256: digest([...manifest.shared, ...entries]),
      lockSha256: digest(lockClosure(packages, [crateName]))
    }
  }
}

/**
 * The files a component's identity hashes. A crate-built Core takes no shared
 * files: what it runs is its own entry files plus its crate closure.
 */
function componentPayload(component, inputs, fileDigests, crateMaterial) {
  const shared = crateMaterial ? [] : [...inputs.sharedFiles].sort()
  const own = [...inputs.components[component], ...(crateMaterial?.crateDirs ?? [])].sort()
  const paths = new Set([...shared, ...own])
  const files = [...fileDigests.entries()]
    .filter(
      ([path]) => paths.has(path) || [...paths].some((prefix) => path.startsWith(`${prefix}/`))
    )
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([path, digest]) => ({ path, sha256: digest }))

  return {
    schemaVersion: ID_SCHEMA_VERSION,
    component,
    files
  }
}

export async function generateComponentIdentities({ root = process.cwd(), inputs }) {
  const normalizedRoot = resolve(root)
  const source = inputs ?? JSON.parse(await readFile(DEFAULT_INPUTS, 'utf8'))
  const { fileDigests, crateMaterial } = await readInputs(normalizedRoot, source)
  const components = {}
  const payloads = {}

  for (const component of COMPONENTS) {
    const payload = componentPayload(component, source, fileDigests, crateMaterial[component])
    if (crateMaterial[component]) payload.cargo = crateMaterial[component].cargo
    if (source.cargo?.scopedComponents?.includes(component)) {
      payload.cargo = await scopedCargoMaterial(
        normalizedRoot,
        source.cargo,
        source.components[component]
      )
    }
    const digest = createHash('sha256').update(JSON.stringify(payload), 'utf8').digest('hex')
    components[component] = {
      buildId: `termul-${component}-v${ID_SCHEMA_VERSION}-sha256:${digest}`
    }
    payloads[component] = payload
  }

  return {
    schemaVersion: ID_SCHEMA_VERSION,
    components,
    inputs: {
      sharedFiles: [...source.sharedFiles].sort(),
      components: Object.fromEntries(
        COMPONENTS.map((component) => [component, [...source.components[component]].sort()])
      )
    },
    payloads
  }
}

/**
 * Why each component's identity differs between two `generateComponentIdentities`
 * results: the input files added, removed or modified, and for scoped Cores
 * the Cargo dependency roots and material that moved. Release logs print this
 * so an unexpected Core replacement names its cause.
 */
export function explainIdentityChange(previous, current) {
  const lines = []
  for (const component of COMPONENTS) {
    if (previous.components[component]?.buildId === current.components[component].buildId) {
      lines.push(`${component}: unchanged`)
      continue
    }
    lines.push(`${component}: changed`)
    const before = new Map(
      (previous.payloads?.[component]?.files ?? []).map((file) => [file.path, file.sha256])
    )
    const after = new Map(current.payloads[component].files.map((file) => [file.path, file.sha256]))
    for (const [path, sha256] of after) {
      if (!before.has(path)) lines.push(`  + ${path}`)
      else if (before.get(path) !== sha256) lines.push(`  ~ ${path}`)
    }
    for (const path of before.keys()) if (!after.has(path)) lines.push(`  - ${path}`)

    const oldCargo = previous.payloads?.[component]?.cargo
    const newCargo = current.payloads[component].cargo
    if (oldCargo || newCargo) {
      const oldRoots = new Set(oldCargo?.roots ?? [])
      const newRoots = new Set(newCargo?.roots ?? [])
      for (const name of newRoots)
        if (!oldRoots.has(name)) lines.push(`  + cargo dependency ${name}`)
      for (const name of oldRoots)
        if (!newRoots.has(name)) lines.push(`  - cargo dependency ${name}`)
      if (oldCargo?.manifestSha256 !== newCargo?.manifestSha256) {
        lines.push('  ~ Cargo.toml (scoped part)')
      }
      if (oldCargo?.lockSha256 !== newCargo?.lockSha256) lines.push('  ~ Cargo.lock (scoped part)')
    }
  }
  return lines
}

/** Identities of `revision`, computed from its own tree and its own input list. */
async function identitiesAtRevision(revision) {
  const directory = await mkdtemp(join(tmpdir(), 'se-component-identity-'))
  try {
    const archive = spawnSync('git', ['archive', '--format=tar', revision], {
      maxBuffer: 1024 * 1024 * 1024
    })
    if (archive.status !== 0) {
      throw new Error(`git archive ${revision} failed: ${archive.stderr.toString().trim()}`)
    }
    const extract = spawnSync('tar', ['-x', '-C', directory], { input: archive.stdout })
    if (extract.status !== 0) throw new Error(`tar failed: ${extract.stderr.toString().trim()}`)
    const inputs = JSON.parse(
      await readFile(join(directory, 'scripts/release/component-build-inputs.json'), 'utf8')
    )
    return await generateComponentIdentities({ root: directory, inputs })
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}

async function runCli(argv) {
  let inputsPath = DEFAULT_INPUTS
  let outputPath = null
  let explainRevision = null
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--inputs') inputsPath = resolve(argv[++index])
    else if (argument === '--output') outputPath = resolve(argv[++index])
    else if (argument === '--explain-against') explainRevision = argv[++index]
    else if (argument === '--help') {
      process.stdout.write(
        'Usage: component-build-id.mjs [--inputs <path>] [--output <path>] [--explain-against <git-rev>]\n'
      )
      return
    } else throw new Error(`Unknown argument: ${argument}`)
  }

  const result = await generateComponentIdentities({
    root: process.cwd(),
    inputs: JSON.parse(await readFile(inputsPath, 'utf8'))
  })
  if (explainRevision) {
    const previous = await identitiesAtRevision(explainRevision)
    process.stdout.write(
      `Component identity changes since ${explainRevision}:\n${explainIdentityChange(previous, result).join('\n')}\n`
    )
    return
  }
  const serialized = `${JSON.stringify(result, null, 2)}\n`
  if (outputPath) await writeFile(outputPath, serialized)
  else process.stdout.write(serialized)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runCli(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
