import { describe, expect, it } from 'vitest'
import { readTerminalCellSize } from './terminal-cell-size'

function terminalWithCell(cell: { width?: number; height?: number } | undefined): object {
  return { _core: { _renderService: { dimensions: { css: { cell } } } } }
}

describe('readTerminalCellSize', () => {
  it('rounds the measured CSS cell to whole pixels', () => {
    expect(readTerminalCellSize(terminalWithCell({ width: 7.8, height: 17.2 }))).toEqual({
      width: 8,
      height: 17
    })
  })

  it('returns undefined until the renderer has measured a real cell', () => {
    expect(readTerminalCellSize({})).toBeUndefined()
    expect(readTerminalCellSize(terminalWithCell(undefined))).toBeUndefined()
    expect(readTerminalCellSize(terminalWithCell({ width: 0, height: 0 }))).toBeUndefined()
    expect(readTerminalCellSize(terminalWithCell({ width: 8, height: 0.4 }))).toBeUndefined()
    expect(
      readTerminalCellSize(terminalWithCell({ width: Number.NaN, height: 17 }))
    ).toBeUndefined()
    expect(
      readTerminalCellSize(terminalWithCell({ width: Number.POSITIVE_INFINITY, height: 17 }))
    ).toBeUndefined()
  })

  it('drops a cell the host could not carry as u16', () => {
    expect(readTerminalCellSize(terminalWithCell({ width: 70000, height: 17 }))).toBeUndefined()
  })
})
