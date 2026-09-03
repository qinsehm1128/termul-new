/**
 * T-H11 — renderer-internal wire names, checked between the file that emits
 * them and the files that consume them.
 *
 * Three contracts, all of the same shape: one module owns a brand-prefixed
 * string, and a *different* module has to agree with it at runtime with no
 * compiler edge in between (a DOM event name matched by string; a CSS custom
 * property written by one file and read by another). Nothing type-checks these
 * pairings, and the existing tests do not close them either — most visibly
 * `use-osk-viewport.test.ts`, which asserts the hook writes
 * `--termul-keyboard-height` and then asserts it can read that same literal
 * back out of the style object it just watched being set. A rename that missed
 * the consumer keeps it green.
 *
 * So nothing here is written down as a literal. Each name is extracted from
 * its declaring file on disk, and the consumers are found by scanning the
 * renderer tree on disk. The comparison is always "the set one side produces"
 * versus "the set the other side consumes"; the only brand string this file
 * knows is the prefix it asks the brand seam for.
 *
 * On the CSS-property contract, note what the scan actually finds:
 * `--termul-keyboard-height` is written in `hooks/use-osk-viewport.ts` and read
 * in `components/chat/AgentChatPanel.tsx`. It is *not* in `index.css` —
 * `index.css` declares no brand-prefixed custom property at all. The stylesheet
 * is still read from disk and folded into the consumer scan so the pairing
 * follows the property if it ever moves into CSS, and so a brand-prefixed
 * property declared only in the stylesheet shows up as unwritten.
 */
import { readdirSync, readFileSync } from 'node:fs'
import { extname, join, relative, sep } from 'node:path'
import { __resetBrandCanonicalOverride, brandCanonical } from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'

const repoRoot = process.cwd()
const RENDERER_ROOT = 'src/renderer'
const SOURCE_EXTENSIONS = new Set(['.ts', '.tsx', '.css'])
const SKIPPED_DIRECTORIES = new Set(['__tests__', '__fixtures__', 'locales', 'node_modules'])

const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * Every production renderer source file, as repo-relative POSIX paths.
 *
 * Test files are excluded deliberately: a literal inside a test is the test's
 * own business, and counting them would let a test file satisfy a "someone
 * consumes this" assertion that production code no longer does.
 */
function listRendererSources(): string[] {
  const absoluteRoot = join(repoRoot, RENDERER_ROOT)
  const found: string[] = []
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const absolute = join(dir, entry.name)
      if (entry.isDirectory()) {
        if (!SKIPPED_DIRECTORIES.has(entry.name)) walk(absolute)
        continue
      }
      if (!SOURCE_EXTENSIONS.has(extname(entry.name))) continue
      if (/\.test\.tsx?$/.test(entry.name)) continue
      found.push(relative(repoRoot, absolute).split(sep).join('/'))
    }
  }
  walk(absoluteRoot)
  return found.sort()
}

const rendererSources = listRendererSources()
const sourceText = new Map(rendererSources.map((path) => [path, read(path)]))

/** Files whose text contains `needle`, excluding the declaring file itself. */
function filesContaining(needle: string, except: string): string[] {
  return rendererSources.filter((path) => path !== except && sourceText.get(path)?.includes(needle))
}

/** The single-quoted string a `export const <name> = '…'` declaration binds. */
function declaredStringConstant(relativePath: string, constantName: string): string {
  const match = read(relativePath).match(
    new RegExp(`export const ${escapeRegExp(constantName)}\\s*=\\s*'([^']*)'`)
  )
  if (!match) throw new Error(`${constantName} not found in ${relativePath}`)
  return match[1]
}

/** Files that call `addEventListener(<constantName>` / `removeEventListener(`. */
function listenerFiles(constantName: string): string[] {
  const pattern = new RegExp(`\\baddEventListener\\(\\s*${escapeRegExp(constantName)}\\b`)
  return rendererSources.filter((path) => pattern.test(sourceText.get(path) ?? ''))
}

/** Files that call `dispatchEvent(new CustomEvent(<constantName>` … ). */
function dispatcherFiles(constantName: string): string[] {
  const pattern = new RegExp(
    `dispatchEvent\\(\\s*\\n?\\s*new CustomEvent(?:<[^>]*>)?\\(\\s*\\n?\\s*${escapeRegExp(constantName)}\\b`
  )
  return rendererSources.filter((path) => pattern.test(sourceText.get(path) ?? ''))
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

/**
 * Shared body for the two custom-DOM-event contracts.
 *
 * The property under test is that the wire name has exactly *one* spelling on
 * disk. A dispatcher and a listener that each carry their own copy of
 * `'termul:color-theme-changed'` type-check, run, and pass every existing test
 * — right up until a rename touches one copy and not the other. Requiring the
 * literal to exist in exactly one file, and every other participant to go
 * through the exported symbol, makes that divergence impossible to introduce
 * without turning this red.
 */
function describeEventContract(label: string, declaringFile: string, constantName: string): void {
  describe(label, () => {
    const eventName = declaredStringConstant(declaringFile, constantName)

    it('carries the canonical event prefix', () => {
      expect(eventName.startsWith(brandCanonical().eventPrefix)).toBe(true)
    })

    it('is spelled out in exactly one production file', () => {
      // Any second file holding the literal is a copy that can drift.
      expect(filesContaining(`'${eventName}'`, declaringFile)).toEqual([])
    })

    it('has both ends of the wire, in more than one file', () => {
      // A dispatcher with no listener (or the reverse) is a wire with one end
      // unplugged, which is what a half-applied rename leaves behind.
      const dispatchers = dispatcherFiles(constantName)
      const listeners = listenerFiles(constantName)
      expect(dispatchers.length).toBeGreaterThan(0)
      expect(listeners.length).toBeGreaterThan(0)
      expect(new Set([...dispatchers, ...listeners]).size).toBeGreaterThan(1)
    })

    it('reaches every participant through an import of the declaring constant', () => {
      // Without this, a file could re-declare a same-named local constant and
      // satisfy the checks above while listening on a different string.
      // `import { X } from …` binds it; so does the barrel's
      // `export { X } from './types'`. A local `const X = …` does not.
      const importsSymbol = new RegExp(
        `(?:import|export)\\s+(?:type\\s+)?\\{[^}]*\\b${escapeRegExp(constantName)}\\b[^}]*\\}\\s*from`
      )
      const withoutImport = filesContaining(constantName, declaringFile).filter(
        (path) => !importsSymbol.test(sourceText.get(path) ?? '')
      )
      expect(withoutImport).toEqual([])
    })
  })
}

describeEventContract(
  'COLOR_THEME_CHANGED_EVENT',
  'src/renderer/lib/themes/types.ts',
  'COLOR_THEME_CHANGED_EVENT'
)

describeEventContract(
  'AGENT_SKILLS_CHANGED_EVENT',
  'src/renderer/lib/agent-skills-events.ts',
  'AGENT_SKILLS_CHANGED_EVENT'
)

describe('brand-prefixed CSS custom properties', () => {
  const prefix = brandCanonical().cssVarPrefix
  const writePattern = new RegExp(
    `setProperty\\(\\s*['"](${escapeRegExp(prefix)}[a-z0-9-]+)['"]`,
    'g'
  )
  const readPattern = new RegExp(`var\\(\\s*(${escapeRegExp(prefix)}[a-z0-9-]+)`, 'g')
  const declarePattern = new RegExp(`^\\s*(${escapeRegExp(prefix)}[a-z0-9-]+)\\s*:`, 'gm')

  const collect = (pattern: RegExp): Map<string, string[]> => {
    const byName = new Map<string, string[]>()
    for (const path of rendererSources) {
      for (const match of (sourceText.get(path) ?? '').matchAll(pattern)) {
        const sites = byName.get(match[1]) ?? []
        sites.push(path)
        byName.set(match[1], sites)
      }
    }
    return byName
  }

  const written = collect(writePattern)
  const readVia = collect(readPattern)
  const declaredInCss = collect(declarePattern)

  it('writes at least one property, so the comparisons below are not vacuous', () => {
    expect([...written.keys()].sort()).not.toEqual([])
  })

  it('matches every written property to a consumer in a different file', () => {
    // `use-osk-viewport.ts` sets the property; something else has to read it,
    // and reading it in the *same* file would not exercise the wire.
    const orphans = [...written.entries()]
      .filter(([name, writers]) => {
        const consumers = [...(readVia.get(name) ?? []), ...(declaredInCss.get(name) ?? [])]
        return !consumers.some((path) => !writers.includes(path))
      })
      .map(([name]) => name)
      .sort()
    expect(orphans).toEqual([])
  })

  it('matches every consumed property back to a writer', () => {
    // The reverse direction: a `var(--termul-…)` that nothing sets renders as
    // the fallback forever, silently.
    const unwritten = [...new Set([...readVia.keys(), ...declaredInCss.keys()])]
      .filter((name) => !written.has(name))
      .sort()
    expect(unwritten).toEqual([])
  })
})
