import type { TerminalCellSize } from '@shared/types/ipc.types'

type TerminalCoreHandle = {
  _renderService?: {
    dimensions?: { css?: { cell?: { width?: number; height?: number } } }
  }
}

// The host carries the cell size as u16; anything outside it would fail the
// whole resize request rather than just the pixel size.
const MAX_CELL_PX = 0xffff

// The range check also rejects NaN and Infinity.
function toCellPx(value: number | undefined): number | undefined {
  const rounded = Math.round(value ?? 0)
  return rounded > 0 && rounded <= MAX_CELL_PX ? rounded : undefined
}

/**
 * xterm's measured CSS cell size, rounded to whole pixels, so the PTY can
 * report a TIOCGWINSZ pixel size for programs that size inline images.
 * `undefined` until the renderer has measured a real cell.
 */
export function readTerminalCellSize(terminal: object): TerminalCellSize | undefined {
  const cell = (terminal as { _core?: TerminalCoreHandle })._core?._renderService?.dimensions?.css
    ?.cell
  const width = toCellPx(cell?.width)
  const height = toCellPx(cell?.height)
  return width !== undefined && height !== undefined ? { width, height } : undefined
}
