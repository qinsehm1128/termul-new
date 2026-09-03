/**
 * T-H20 — the negotiated binary WebSocket subprotocol, across every file that
 * spells it.
 *
 * The subprotocol name is the one string a browser client and the Rust server
 * must agree on byte for byte, and the agreement is enforced by nothing but a
 * string comparison inside the HTTP upgrade handshake. If the two sides drift,
 * `supports_binary_subprotocol` simply returns false: no error, no failed
 * build, just every client silently falling back to the text protocol.
 *
 * It is carried independently in four places — the TypeScript brand seam, the
 * TypeScript protocol module the web client imports, the Rust brand seam, and
 * the Rust `const` the upgrade handler compares against — plus once more as a
 * bare literal inside the `Sec-WebSocket-Protocol` header the Rust test feeds
 * its own negotiation code. That last one matters most: it is the only place
 * the *client's* side of the handshake is written down, and it is a
 * `from_static` string that no compiler ties back to `BINARY_SUBPROTOCOL`.
 *
 * Every value below is therefore extracted from its own file on disk, and the
 * five are compared against each other. Writing the name out here would make
 * this test a copy of its own subject and a repo-wide rename would keep it
 * green while the handshake broke.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { __resetBrandCanonicalOverride, brandCanonical } from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'

const repoRoot = process.cwd()
const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

const PROTOCOL_TYPES_TS = 'src/shared/types/web-terminal-protocol.types.ts'
const TERMINAL_WS_RS = 'src-tauri/src/web/terminal_ws.rs'
const BRAND_RS = 'src-tauri/src/brand.rs'

function extract(relativePath: string, pattern: RegExp, what: string): string {
  const match = read(relativePath).match(pattern)
  if (!match) throw new Error(`${what} not found in ${relativePath}`)
  return match[1]
}

/** `export const WEB_TERMINAL_BINARY_PROTOCOL = '…'` — what the web client offers. */
function typescriptProtocolConstant(): string {
  return extract(
    PROTOCOL_TYPES_TS,
    /export const WEB_TERMINAL_BINARY_PROTOCOL = '([^']*)'/,
    'WEB_TERMINAL_BINARY_PROTOCOL'
  )
}

/** `const BINARY_SUBPROTOCOL: &str = "…";` — what the server accepts. */
function rustSubprotocolConstant(): string {
  return extract(
    TERMINAL_WS_RS,
    /const BINARY_SUBPROTOCOL: &str = "([^"]*)";/,
    'BINARY_SUBPROTOCOL'
  )
}

/**
 * The subprotocol inside the `legacy, <name>` header literal.
 *
 * `Sec-WebSocket-Protocol` is a comma-separated offer list, so the name is
 * taken as the last entry rather than by position in the string.
 */
function headerOfferedSubprotocol(): string {
  const header = extract(
    TERMINAL_WS_RS,
    /HeaderValue::from_static\("(legacy,[^"]*)"\)/,
    'Sec-WebSocket-Protocol offer'
  )
  const offers = header.split(',').map((offer) => offer.trim())
  return offers[offers.length - 1]
}

/**
 * `ws_subprotocol` from the Rust seam's `DEFAULT_CANONICAL`.
 *
 * Anchored past the `DEFAULT_CANONICAL` declaration so the identical field in
 * `LEGACY` above it — which is permanent by design and must *not* move — never
 * satisfies this lookup.
 */
function rustSeamCanonical(): string {
  const source = read(BRAND_RS)
  const start = source.indexOf('pub const DEFAULT_CANONICAL')
  if (start < 0) throw new Error(`DEFAULT_CANONICAL not found in ${BRAND_RS}`)
  const match = source.slice(start).match(/ws_subprotocol: "([^"]*)",/)
  if (!match) throw new Error(`ws_subprotocol not found in ${BRAND_RS} DEFAULT_CANONICAL`)
  return match[1]
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('binary WebSocket subprotocol', () => {
  const sources = {
    'TypeScript brand seam': (): string => brandCanonical().wsSubprotocol,
    'Rust brand seam': rustSeamCanonical,
    'web client protocol module': typescriptProtocolConstant,
    'Rust upgrade handler': rustSubprotocolConstant,
    'Rust handshake header offer': headerOfferedSubprotocol
  }

  it('reads a non-empty name from every carrier', () => {
    // Vacuity guard: a broken extractor would otherwise compare '' to '' and
    // report parity it never checked.
    for (const resolve of Object.values(sources)) {
      expect(resolve().length).toBeGreaterThan(0)
    }
  })

  it('agrees on one name across all five carriers', () => {
    const resolved = Object.fromEntries(
      Object.entries(sources).map(([label, resolve]) => [label, resolve()])
    )
    // The expected side is the *upgrade handler's* value — the one the server
    // actually compares against at runtime — broadcast to every label, so a
    // failure diff names each carrier that drifted rather than just reporting
    // that two unnamed strings differ. Nothing here is a written-down literal.
    const server = rustSubprotocolConstant()
    expect(resolved).toEqual(
      Object.fromEntries(Object.keys(sources).map((label) => [label, server]))
    )
  })
})
