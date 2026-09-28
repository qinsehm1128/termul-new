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

type Inputs = {
  sharedFiles: string[]
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

/** Workspace crates (`termul_foo::` → `termul-foo`) named by a Core's source. */
function referencedCrates(source: string): Set<string> {
  const crates = new Set<string>()
  for (const match of source.matchAll(/\b(termul_[a-z_0-9]+)::/g)) {
    const name = match[1].replaceAll('_', '-')
    if (workspaceCrates.has(name)) crates.add(name)
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

function uncoveredReferences(core: (typeof CORES)[number]): Map<string, string[]> {
  const paths = [...inputs.components[core], ...inputs.sharedFiles]
  const found = new Map<string, string[]>()
  for (const file of inputs.components[core].flatMap(rustFiles)) {
    for (const module of referencedModules(productionSource(file))) {
      if (isCovered(module, paths)) continue
      found.set(module, [...(found.get(module) ?? []), file])
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
    CORES
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

  test.each(CORES)('%s identity covers every workspace crate its code uses', (core) => {
    const paths = [...inputs.components[core], ...inputs.sharedFiles]
    const uncovered = new Set<string>()
    // Shared files run in every Core too (e.g. `core/ipc.rs` re-exports the IPC crate).
    for (const file of paths.flatMap(rustFiles)) {
      for (const name of referencedCrates(productionSource(file))) {
        const dir = `${crateRoot}/${name}`
        if (!paths.some((path) => path === dir || dir.startsWith(`${path}/`))) uncovered.add(name)
      }
    }
    expect([...uncovered]).toEqual([])
  })

  test.each(CORES)('%s acknowledges only modules it still references', (core) => {
    const referenced = uncoveredReferences(core)
    const stale = Object.keys(inputs.acknowledgedExternalModules[core] ?? {}).filter(
      (module) => !referenced.has(module)
    )
    expect(stale).toEqual([])
  })
})
