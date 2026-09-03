/**
 * T-H02 — `ConversationRecordV2.createdBy` must survive the rename in both
 * directions.
 *
 * The record under test is read from `src/__fixtures__/legacy-brand/` instead
 * of being built inline, because an inline `{ createdBy: 'termul' }` is a copy
 * of the parser's own literal: one repo-wide sed rewrites the assertion and the
 * parser in the same stroke, the suite stays green, and every conversation
 * already on a user's disk has silently become unreadable. The fixture is
 * sha256-frozen (`src/__fixtures__/legacy-brand-manifest.test.ts`), so the two
 * sides cannot move together.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from '../brand'
import { parseConversationRecordV2 } from './conversation.types'

const FIXTURE = join(
  process.cwd(),
  'src/__fixtures__/legacy-brand/conversation-createdBy-termul.json'
)

/** The exact bytes a pre-rename install left in its conversation index. */
function recordOnDisk(): Record<string, unknown> {
  return JSON.parse(readFileSync(FIXTURE, 'utf8')) as Record<string, unknown>
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('conversation createdBy across the rename', () => {
  it('still parses the record a pre-rename install wrote to disk', () => {
    // Green today, and that is the point: it goes red the moment the parser's
    // literal is renamed without a compatibility read behind it.
    __setBrandCanonicalOverride({ createdBy: 'se-manager' })
    const onDisk = recordOnDisk()
    expect(onDisk.createdBy).toBe(LEGACY.createdBy)

    const parsed = parseConversationRecordV2(onDisk)
    expect(parsed.conversationId).toBe(onDisk.conversationId)
    expect(parsed.lastSeq).toBe(onDisk.lastSeq)
  })

  // Was a Wave-4 ledger entry (`test.fails`): `parseConversationRecordV2` used
  // to compare `createdBy` against a hardcoded 'termul' and consult neither
  // `brandCanonical()` nor `LEGACY`, so a record written *after* the flip was
  // rejected as invalid. T-A01 made the parser read both, so the `.fails` is
  // gone and this stands as a live guard against the widening being undone.
  it('accepts a record carrying the post-rename createdBy', () => {
    __setBrandCanonicalOverride({ createdBy: 'se-manager' })
    const onDisk = recordOnDisk()
    const writtenToday = { ...onDisk, createdBy: brandCanonical().createdBy }

    const parsed = parseConversationRecordV2(writtenToday)
    expect(parsed.createdBy).toBe(brandCanonical().createdBy)
    expect(parsed.lastSeq).toBe(onDisk.lastSeq)
  })

  // T-H22 — the TypeScript half of "the wire value must not be frozen at
  // module load". `brand.ts` warns about exactly this: a value captured into a
  // top-level `const` freezes before a test can override it, so the accepting
  // parser Wave 4 writes would consult a snapshot rather than the seam, the
  // test above would go green, and nothing would actually have changed.
  //
  // Vacuously true today — `conversation.types.ts` compares against a literal
  // and imports neither symbol — and that is stated rather than hidden. It
  // becomes load-bearing the moment the parser starts reading the seam, which
  // is the change it exists to constrain.
  it('never freezes the createdBy values into a module-level constant', () => {
    const source = readFileSync(
      join(process.cwd(), 'src/shared/types/conversation.types.ts'),
      'utf8'
    )
    const moduleScopeCaptures = source
      .split('\n')
      .filter((line) => /^(export\s+)?(const|let|var)\s/.test(line))
      .filter((line) => line.includes('brandCanonical(') || /\bLEGACY\.createdBy\b/.test(line))

    expect(moduleScopeCaptures).toEqual([])
  })

  // And the seam really is live at call time, which is what makes the guard
  // above worth having: the override set inside this test body is what a call
  // made now observes.
  it('resolves the canonical createdBy at call time, not at import time', () => {
    // The injected value has to differ from the shipped one or the test proves
    // nothing. Now that the contract has flipped, the legacy value is the one
    // spelling `brandCanonical()` never returns on its own.
    const before = brandCanonical().createdBy
    __setBrandCanonicalOverride({ createdBy: LEGACY.createdBy })
    expect(brandCanonical().createdBy).not.toBe(before)
    expect(brandCanonical().createdBy).toBe(LEGACY.createdBy)
    // LEGACY is immovable by design — it is what is already on disk.
    expect(LEGACY.createdBy).toBe(recordOnDisk().createdBy)
  })
})
