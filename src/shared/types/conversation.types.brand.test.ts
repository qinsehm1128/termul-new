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
import { afterEach, describe, expect, it, test } from 'vitest'
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

  // LEDGER (Wave 4) — expected failure. `parseConversationRecordV2` compares
  // `createdBy` against a hardcoded 'termul' and consults neither
  // `brandCanonical()` nor `LEGACY`, so a record written *after* the flip is
  // rejected as invalid. Delete this test, `.fails` and all, once the parser
  // accepts the canonical value alongside the legacy one.
  test.fails('accepts a record carrying the post-rename createdBy', () => {
    __setBrandCanonicalOverride({ createdBy: 'se-manager' })
    const onDisk = recordOnDisk()
    const writtenToday = { ...onDisk, createdBy: brandCanonical().createdBy }

    const parsed = parseConversationRecordV2(writtenToday)
    expect(parsed.createdBy).toBe(brandCanonical().createdBy)
    expect(parsed.lastSeq).toBe(onDisk.lastSeq)
  })
})
