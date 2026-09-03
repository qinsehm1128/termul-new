/**
 * T-H03 — the ACP plan fence language must survive the rename in both
 * directions.
 *
 * The transcript is read from `src/__fixtures__/legacy-brand/` rather than
 * inlined, because an inline ```termul-plan fence is a copy of the extractor's
 * own regexp literal: one repo-wide sed rewrites the fixture-that-isn't, the
 * regexp and the renderer's `language ===` comparison together, the suite stays
 * green, and every plan snapshot already persisted in a user's chat history
 * quietly stops rendering. The fixture is sha256-frozen
 * (`src/__fixtures__/legacy-brand-manifest.test.ts`), so the two sides cannot
 * move together.
 *
 * Both surfaces the fence language crosses are covered: `extractTermulPlanFence
 * Json` (rehydrate path) and the `language === 'termul-plan'` dispatch in
 * `ChatMarkdownCode` (render path). A rename that fixes one and misses the
 * other leaves the plan unreadable just as surely.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from '@shared/brand'
import { render } from '@testing-library/react'
import { createContext, createElement } from 'react'
import { afterEach, describe, expect, it, test, vi } from 'vitest'

const { useIsCodeFenceIncompleteMock } = vi.hoisted(() => ({
  useIsCodeFenceIncompleteMock: vi.fn(() => false)
}))

vi.mock('streamdown', () => {
  const StreamdownContext = createContext({ lineNumbers: false, isAnimating: false })
  return {
    StreamdownContext,
    Streamdown: () => null,
    useIsCodeFenceIncomplete: useIsCodeFenceIncompleteMock,
    // A stand-in for the fenced-code branch: its presence is exactly the
    // symptom of a plan fence that was NOT recognized as a plan.
    CodeBlock: (props: { code?: string; children?: React.ReactNode }) =>
      createElement('div', { 'data-testid': 'code-block', 'data-code': props.code ?? '' })
  }
})

vi.mock('@streamdown/mermaid', () => ({ mermaid: () => {} }))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn(() => Promise.resolve())
}))

import { ChatMarkdownCode } from '@/components/chat/chat-markdown-code'
import { TooltipProvider } from '@/components/ui/tooltip'
import { extractSePlanFenceJson } from './acp-store'

const FIXTURE = join(process.cwd(), 'src/__fixtures__/legacy-brand/chat-transcript-termul-plan.md')

/** The exact bytes a pre-rename install persisted for an assistant turn. */
function transcriptOnDisk(): string {
  return readFileSync(FIXTURE, 'utf8')
}

/** The same transcript as an agent would emit it *after* the flip. */
function transcriptWrittenToday(): string {
  return transcriptOnDisk().replaceAll(
    `\`\`\`${LEGACY.planFence}`,
    `\`\`\`${brandCanonical().planFence}`
  )
}

/**
 * The plan entries carried by the fixture. The fixture wraps them in an
 * `{ entries: [...] }` envelope, so `parseSePlanFence` (which requires a
 * top-level array) is not the surface the fence *language* governs —
 * extraction is.
 */
function entriesOnDisk(): unknown[] {
  const json = extractSePlanFenceJson(transcriptOnDisk())
  if (json === null) throw new Error('fixture transcript carries no plan fence')
  const payload = JSON.parse(json) as { entries?: unknown[] }
  if (!Array.isArray(payload.entries)) throw new Error('fixture plan payload has no entries')
  return payload.entries
}

/**
 * `ChatMarkdownCode` treats the presence of `data-block` as "this is a fenced
 * block, not inline code". `createElement` does not get JSX's `data-*` escape
 * hatch, so it is spread in rather than written as a literal property.
 */
const BLOCK_MARKER = { 'data-block': true }

function renderFence(language: string, code: string): ReturnType<typeof render> {
  return render(
    createElement(
      TooltipProvider,
      null,
      createElement(
        ChatMarkdownCode,
        { className: `language-${language}`, node: { value: code }, ...BLOCK_MARKER },
        code
      )
    )
  )
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('ACP plan fence across the rename', () => {
  it('still extracts the plan a pre-rename install persisted', () => {
    // Green today, and that is the point: it goes red the moment the fence
    // language is renamed without a compatibility read behind it.
    __setBrandCanonicalOverride({ planFence: 'se-plan' })
    const json = extractSePlanFenceJson(transcriptOnDisk())

    expect(json).not.toBeNull()
    expect(transcriptOnDisk()).toContain(`\`\`\`${LEGACY.planFence}`)
    expect(entriesOnDisk()).toHaveLength(3)
  })

  it('still renders a persisted legacy fence as a plan, not a code block', () => {
    __setBrandCanonicalOverride({ planFence: 'se-plan' })
    const { getByText, queryByTestId } = renderFence(
      LEGACY.planFence,
      JSON.stringify(entriesOnDisk())
    )

    expect(queryByTestId('code-block')).toBeNull()
    expect(getByText('Reproduce the webhook retry storm')).toBeInTheDocument()
  })

  // LEDGER (Wave 4) — expected failure. The extractor's regexp hardcodes
  // `termul-plan` and consults neither `brandCanonical()` nor `LEGACY`, so a
  // fence emitted after the flip is invisible to the rehydrate path. Delete
  // this test, `.fails` and all, once the extractor accepts the canonical
  // language alongside the legacy one.
  test.fails('extracts a plan fence carrying the post-rename language', () => {
    __setBrandCanonicalOverride({ planFence: 'se-plan' })
    const json = extractSePlanFenceJson(transcriptWrittenToday())

    expect(json).not.toBeNull()
    expect(json).toBe(extractSePlanFenceJson(transcriptOnDisk()))
  })

  // LEDGER (Wave 4) — expected failure. `ChatMarkdownCode` compares `language`
  // against a hardcoded 'termul-plan', so a fence emitted after the flip falls
  // through to the generic code block. Delete this test, `.fails` and all, once
  // the dispatch reads the canonical language.
  test.fails('renders a post-rename plan fence as a plan, not a code block', () => {
    __setBrandCanonicalOverride({ planFence: 'se-plan' })
    const { getByText, queryByTestId } = renderFence(
      brandCanonical().planFence,
      JSON.stringify(entriesOnDisk())
    )

    expect(queryByTestId('code-block')).toBeNull()
    expect(getByText('Reproduce the webhook retry storm')).toBeInTheDocument()
  })
})
