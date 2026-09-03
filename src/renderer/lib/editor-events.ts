/**
 * Window event asking the open code editor to scroll a file to a line.
 *
 * Dispatched from the file explorer's search results, listened for in
 * `CodeEditor`. The name lives here rather than as a literal at each end
 * because a DOM event name is matched by string with no compiler edge in
 * between: two copies type-check, run, and pass every test right up until a
 * rename touches one of them, at which point "reveal line" stops working with
 * no error anywhere. `brand-wire-names.parity.test.ts` enforces the single
 * spelling.
 */
export const EDITOR_REVEAL_LINE_EVENT = 'se:reveal-line'

/** What a reveal-line request carries, on either transport below. */
export interface EditorRevealLineDetail {
  filePath: string
  lineNumber: number
  searchTerm?: string
}

/**
 * Window global holding a reveal-line request whose target editor has not
 * mounted yet.
 *
 * The event above only reaches an editor that is already listening. Opening a
 * file from search mounts the editor *after* the dispatch, so the file explorer
 * also parks the request here and the editor drains it on first render. Same
 * handoff, second transport — and the same hazard, one step worse: a window
 * property has no listener to go missing, so a half-applied rename leaves the
 * writer parking under one key and the reader draining another, and "jump to
 * line in a not-yet-open tab" silently does nothing.
 *
 * Held here for the same reason as the event name, and guarded the same way by
 * `brand-wire-names.parity.test.ts`. The prefix is the brand module's
 * `domGlobalPrefix`.
 */
export const PENDING_REVEAL_LINE_GLOBAL = '__sePendingRevealLine'

/** The `window` shape {@link PENDING_REVEAL_LINE_GLOBAL} implies. */
export type PendingRevealLineWindow = Partial<
  Record<typeof PENDING_REVEAL_LINE_GLOBAL, EditorRevealLineDetail>
>
