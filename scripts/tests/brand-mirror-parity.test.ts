import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

/**
 * `src/shared/brand.ts` and `src-tauri/src/brand.rs` hold the same two tables in
 * two languages. Nothing kept them equal, and they drifted: `plan_fence` said
 * `termul-plan` in Rust while TypeScript — the side that actually writes the
 * fence — had already flipped to `se-plan`, and seven Rust-owned contracts
 * (keychain services, MCP name, skill name/marker/manifest key, frp proxy name)
 * had flipped in Rust while the TypeScript mirror still advertised the legacy
 * spelling.
 *
 * None of it was caught, because every existing parity test pins one specific
 * contract (bundle identifier, ws subprotocol, env names, iOS legacy keys) and
 * a field nobody thought to pin is a field nobody notices.
 *
 * Parsed from source text rather than imported: `brand.rs` cannot be imported
 * from vitest, and reading both as text is what makes the comparison symmetric.
 */

const TS_SOURCE = readFileSync('src/shared/brand.ts', 'utf8')
const RS_SOURCE = readFileSync('src-tauri/src/brand.rs', 'utf8')

function camelToSnake(name: string): string {
  return name.replace(/(?<!^)(?=[A-Z])/g, '_').toLowerCase()
}

function parseTsTable(declaration: string): Record<string, string> {
  const start = TS_SOURCE.indexOf(declaration)
  if (start < 0) throw new Error(`TypeScript table not found: ${declaration}`)
  const body = TS_SOURCE.slice(start, TS_SOURCE.indexOf('\n}', start))
  const table: Record<string, string> = {}
  for (const [, key, value] of body.matchAll(/^\s{2}(\w+):\s*'(.*)',?$/gm)) {
    table[key] = value
  }
  return table
}

function parseRustTable(name: string): Record<string, string> {
  const marker = `pub const ${name}: BrandCanonical = BrandCanonical {`
  const start = RS_SOURCE.indexOf(marker)
  if (start < 0) throw new Error(`Rust table not found: ${name}`)
  const body = RS_SOURCE.slice(start, RS_SOURCE.indexOf('\n};', start))
  const table: Record<string, string> = {}
  for (const [, key, value] of body.matchAll(/^\s{4}(\w+):\s*"(.*)",?$/gm)) {
    table[key] = value
  }
  return table
}

describe('brand table parity between TypeScript and Rust', () => {
  it.each([
    ['LEGACY', 'export const LEGACY: BrandCanonical = {', 'LEGACY'],
    ['canonical', 'const DEFAULT_CANONICAL: BrandCanonical = {', 'DEFAULT_CANONICAL']
  ])('%s holds the same value for every shared field', (_label, tsDecl, rsName) => {
    const ts = parseTsTable(tsDecl)
    const rs = parseRustTable(rsName)
    expect(Object.keys(ts).length).toBeGreaterThan(20)
    expect(Object.keys(rs).length).toBeGreaterThan(20)

    const divergent = Object.entries(ts)
      .map(([key, value]) => ({ key, ts: value, rs: rs[camelToSnake(key)] }))
      .filter((row) => row.rs !== undefined && row.rs !== row.ts)

    expect(divergent).toEqual([])
  })

  /**
   * The two tables are deliberately not the same shape: themes, DOM events and
   * CSS variables never leave the renderer, and the iOS keys are read by the
   * companion app. Pinned as a list so a field that goes Rust-less by accident
   * is a red test rather than a silent hole in the parity check above.
   */
  it('only the renderer-only and iOS-only fields lack a Rust counterpart', () => {
    const ts = parseTsTable('const DEFAULT_CANONICAL: BrandCanonical = {')
    const rs = parseRustTable('DEFAULT_CANONICAL')
    const tsOnly = Object.keys(ts).filter((key) => !(camelToSnake(key) in rs))
    expect(tsOnly.sort()).toEqual(
      [
        'cssVarPrefix',
        'eventPrefix',
        'iosCacheDir',
        'iosDefaultsPrefix',
        'themeFamilyLight',
        'themeId',
        'urlOpenMode'
      ].sort()
    )
  })
})
