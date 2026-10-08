import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, test } from 'vitest'

// A Core keeps running across a GUI update only while its build id is
// unchanged. The id hashes the files listed for that Core, so every module the
// Core actually runs must be listed — otherwise a changed Core is judged
// current and the stale process is adopted. This guard fails when a Core's
// input files start referencing a top-level module that is neither listed nor
// explicitly acknowledged (with a reason) as never running in that Core.

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '../..')
const srcRoot = 'src-tauri/src'
const CORES = ['terminalCore', 'acpCore'] as const
// Cores built inside the app crate, whose input files are listed by hand.
// The Terminal Core is built from its own crate; Cargo.lock names its inputs.
const SCOPED_CORES = ['acpCore'] as const

type Inputs = {
  sharedFiles: string[]
  cargo: { lock: string; crateComponents: Record<string, string> }
  components: Record<string, string[]>
  acknowledgedExternalModules: Record<string, Record<string, string>>
}

const inputs: Inputs = JSON.parse(
  readFileSync(join(repoRoot, 'scripts/release/component-build-inputs.json'), 'utf8')
)

const topLevelModules = new Set(
  readdirSync(join(repoRoot, srcRoot))
    .map((entry) => entry.replace(/\.rs$/, ''))
    .filter((entry) => entry !== 'lib' && entry !== 'main')
)

function rustFiles(path: string): string[] {
  const absolute = join(repoRoot, path)
  if (!existsSync(absolute)) return []
  if (statSync(absolute).isDirectory()) {
    return readdirSync(absolute).flatMap((entry) => rustFiles(join(path, entry)))
  }
  return path.endsWith('.rs') ? [path] : []
}

/** Drop inline `#[cfg(test)] mod … {` blocks and test-only files. */
function productionSource(path: string): string {
  if (/(^|\/)tests?\//.test(path) || /(tests?|_test)\.rs$/.test(path)) return ''
  const lines = readFileSync(join(repoRoot, path), 'utf8').split('\n')
  for (let index = 0; index < lines.length - 1; index += 1) {
    if (
      lines[index].trim() === '#[cfg(test)]' &&
      /^\s*(pub(\(crate\))? )?mod \w+\s*\{/.test(lines[index + 1])
    ) {
      return lines.slice(0, index).join('\n')
    }
  }
  return lines.join('\n')
}

const crateRoot = 'src-tauri/crates'
const workspaceCrates = new Set(
  existsSync(join(repoRoot, crateRoot)) ? readdirSync(join(repoRoot, crateRoot)) : []
)

/**
 * `lib.rs` re-exports crate modules at their old paths
 * (`use se_foundation::host_admission;`), so `crate::host_admission`
 * in a Core's source still means the `se-foundation` crate.
 */
const reexportedModules = new Map<string, string>()
for (const match of readFileSync(join(repoRoot, srcRoot, 'lib.rs'), 'utf8').matchAll(
  /^(?:pub(?:\(crate\))? )?use (se_[a-z_0-9]+)(?:::([a-z_0-9]+))?(?: as ([a-z_0-9]+))?;/gm
)) {
  const alias = match[3] ?? match[2]
  if (alias) reexportedModules.set(alias, match[1].replaceAll('_', '-'))
}

/** Workspace crates a Core's source uses, directly or through a `crate::` re-export. */
function referencedCrates(source: string): Set<string> {
  const crates = new Set<string>()
  for (const match of source.matchAll(/\b(se_[a-z_0-9]+)::/g)) {
    const name = match[1].replaceAll('_', '-')
    if (workspaceCrates.has(name)) crates.add(name)
  }
  for (const match of source.matchAll(/crate::([a-z_0-9]+)/g)) {
    const name = reexportedModules.get(match[1])
    if (name && workspaceCrates.has(name)) crates.add(name)
  }
  return crates
}

function referencedModules(source: string): Set<string> {
  const modules = new Set<string>()
  for (const match of source.matchAll(/crate::([a-z_0-9]+)/g)) modules.add(match[1])
  // `use crate::{a::X, b::{Y, Z}};` names each top-level module once.
  for (const group of source.matchAll(/crate::\{([\s\S]*?)\};/g)) {
    for (const match of group[1].matchAll(/(?:^|[{,])\s*([a-z_0-9]+)::/g)) modules.add(match[1])
  }
  return new Set([...modules].filter((module) => topLevelModules.has(module)))
}

function isCovered(module: string, paths: string[]): boolean {
  const candidates = [`${srcRoot}/${module}`, `${srcRoot}/${module}.rs`]
  return candidates.some((candidate) =>
    paths.some(
      (path) =>
        path === candidate || path.startsWith(`${candidate}/`) || candidate.startsWith(`${path}/`)
    )
  )
}

const coreDir = `${srcRoot}/core`

/**
 * `core/mod.rs` re-exports crate modules as `core` siblings
 * (`pub use se_terminal_core::{handles, terminal};`), so `super::terminal`
 * in a `core/` file means that crate.
 */
const coreReexports = new Map<string, string>()
for (const match of readFileSync(join(repoRoot, coreDir, 'mod.rs'), 'utf8').matchAll(
  /^pub use (se_[a-z_0-9]+)::\{?([a-z_0-9, ]+)\}?;/gm
)) {
  for (const name of match[2].split(',').map((part) => part.trim())) {
    coreReexports.set(name, match[1].replaceAll('_', '-'))
  }
}

const coreModules = new Set(
  readdirSync(join(repoRoot, coreDir))
    .filter((entry) => entry.endsWith('.rs') && entry !== 'mod.rs')
    .map((entry) => entry.replace(/\.rs$/, ''))
)

/**
 * Sibling `core/` modules a file inside `core/` uses (`super::launcher::…`,
 * `crate::core::launcher::…`). `core/` is one top-level module, so the
 * `crate::` check above sees it as covered as soon as any `core/` file is
 * listed; this keeps a Core from quietly depending on a GUI-only sibling.
 */
function referencedCoreSiblings(file: string, source: string): Set<string> {
  const siblings = new Set<string>()
  if (!file.startsWith(`${coreDir}/`)) return siblings
  for (const match of source.matchAll(/\b(?:super|crate::core)::([a-z_0-9]+)/g)) {
    if (coreModules.has(match[1])) siblings.add(match[1])
  }
  return siblings
}

/** Workspace crates a file uses through `core/mod.rs` re-exports. */
function referencedCoreCrates(file: string, source: string): Set<string> {
  const crates = new Set<string>()
  if (!file.startsWith(`${coreDir}/`)) return crates
  for (const match of source.matchAll(/\b(?:super|crate::core)::([a-z_0-9]+)/g)) {
    const name = coreReexports.get(match[1])
    if (name) crates.add(name)
  }
  return crates
}

function uncoveredReferences(core: (typeof SCOPED_CORES)[number]): Map<string, string[]> {
  const paths = [...inputs.components[core], ...inputs.sharedFiles]
  const found = new Map<string, string[]>()
  const note = (key: string, file: string) => found.set(key, [...(found.get(key) ?? []), file])
  for (const file of inputs.components[core].flatMap(rustFiles)) {
    const source = productionSource(file)
    for (const module of referencedModules(source)) {
      if (!isCovered(module, paths)) note(module, file)
    }
    for (const sibling of referencedCoreSiblings(file, source)) {
      if (!paths.includes(`${coreDir}/${sibling}.rs`)) note(`core::${sibling}`, file)
    }
  }
  return found
}

describe('component build inputs', () => {
  test.each(CORES)('%s lists every path it names', (core) => {
    const missing = inputs.components[core].filter((path) => !existsSync(join(repoRoot, path)))
    expect(missing).toEqual([])
  })

  test.each(
    SCOPED_CORES
  )('%s identity covers every module its code references, or acknowledges why not', (core) => {
    const acknowledged = inputs.acknowledgedExternalModules[core] ?? {}
    const unaccounted = [...uncoveredReferences(core)]
      .filter(([module]) => !(module in acknowledged))
      .map(([module, files]) => `${module} <- ${files.join(', ')}`)
    // Fix by adding the module's path to components.${core} when the Core
    // runs it, or to acknowledgedExternalModules.${core} with the reason it
    // never runs there.
    expect(unaccounted).toEqual([])
  })

  test('reads crate modules that lib.rs re-exports at their old paths', () => {
    expect(reexportedModules.get('host_admission')).toBe('se-foundation')
    expect(referencedCrates('crate::host_admission::HostAdmission::global()')).toEqual(
      new Set(['se-foundation'])
    )
  })

  test('reads crate modules that core/mod.rs re-exports as siblings', () => {
    expect(coreReexports.get('terminal')).toBe('se-terminal-core')
    expect(referencedCoreCrates(`${coreDir}/acp.rs`, 'use super::terminal::Client;')).toEqual(
      new Set(['se-terminal-core'])
    )
  })

  test.each(SCOPED_CORES)('%s identity covers every workspace crate its code uses', (core) => {
    const paths = [...inputs.components[core], ...inputs.sharedFiles]
    const uncovered = new Set<string>()
    // Shared files run in every Core too (e.g. `core/mod.rs` re-exports the Terminal Core crate).
    for (const file of paths.flatMap(rustFiles)) {
      const source = productionSource(file)
      for (const name of [...referencedCrates(source), ...referencedCoreCrates(file, source)]) {
        const dir = `${crateRoot}/${name}`
        if (!paths.some((path) => path === dir || dir.startsWith(`${path}/`))) uncovered.add(name)
      }
    }
    expect([...uncovered]).toEqual([])
  })

  test.each(SCOPED_CORES)('%s acknowledges only modules it still references', (core) => {
    const referenced = uncoveredReferences(core)
    const stale = Object.keys(inputs.acknowledgedExternalModules[core] ?? {}).filter(
      (module) => !referenced.has(module)
    )
    expect(stale).toEqual([])
  })

  test.each(
    Object.entries(inputs.cargo.crateComponents)
  )('%s entry links only its own crate, not the app library', (core, crate) => {
    // The executable's identity is its crate's Cargo.lock closure. Code that
    // reached into the app library would run without being hashed.
    const crateIdent = crate.replaceAll('-', '_')
    const entries = inputs.components[core]
      .flatMap(rustFiles)
      .filter((file) => file.startsWith(`${srcRoot}/`))
    expect(entries.length).toBeGreaterThan(0)
    for (const file of entries) {
      const source = productionSource(file)
      expect(source, file).not.toMatch(/\bcrate::|\bse_manager_lib\b/)
      expect(source, file).toContain(`${crateIdent}::`)
    }
  })
})
