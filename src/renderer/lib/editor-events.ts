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
