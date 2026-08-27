import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CompanionTerminalGeometryContext } from '@/hooks/use-companion-terminal-geometry'
import {
  DEFAULT_COMPANION_TERMINAL_TEXT_SCALE,
  setCompanionTerminalTextScale
} from '@/lib/companion-terminal-text-scale'
import { MobileTerminalControls } from './MobileTerminalControls'

const { write, readText } = vi.hoisted(() => ({
  write: vi.fn(),
  readText: vi.fn()
}))

vi.mock('@/lib/terminal-api', () => ({
  terminalApi: { write }
}))
vi.mock('@/lib/clipboard-api', () => ({
  clipboardApi: { readText }
}))

describe('MobileTerminalControls', () => {
  beforeEach(() => {
    write.mockReset()
    readText.mockReset()
    write.mockResolvedValue({ success: true, data: undefined })
    setCompanionTerminalTextScale(DEFAULT_COMPANION_TERMINAL_TEXT_SCALE)
  })

  it('writes terminal escape/control sequences', () => {
    render(<MobileTerminalControls terminalId="pty-1" />)
    fireEvent.click(screen.getByText('Esc'))
    fireEvent.click(screen.getByText('Ctrl+C'))
    fireEvent.click(screen.getByText('↑'))
    expect(write).toHaveBeenNthCalledWith(1, 'pty-1', '\u001b')
    expect(write).toHaveBeenNthCalledWith(2, 'pty-1', '\u0003')
    expect(write).toHaveBeenNthCalledWith(3, 'pty-1', '\u001b[A')
  })

  it('nudges the companion text scale', () => {
    render(<MobileTerminalControls terminalId="pty-1" />)
    fireEvent.click(screen.getByLabelText('Smaller terminal text'))
    expect(screen.getByLabelText('Terminal text size 100 percent')).toBeInTheDocument()
    fireEvent.click(screen.getByLabelText('Larger terminal text'))
    fireEvent.click(screen.getByLabelText('Larger terminal text'))
    expect(screen.getByLabelText('Terminal text size 150 percent')).toBeInTheDocument()
  })

  it('toggles phone and desktop layout', () => {
    const setPreferredMode = vi.fn()
    render(
      <CompanionTerminalGeometryContext.Provider
        value={{
          surfaceActive: true,
          preferredMode: 'phone',
          keyboardOpen: false,
          setPreferredMode
        }}
      >
        <MobileTerminalControls terminalId="pty-1" />
      </CompanionTerminalGeometryContext.Provider>
    )
    fireEvent.click(screen.getByLabelText('Use desktop size'))
    expect(setPreferredMode).toHaveBeenCalledWith('desktop')
  })

  it('pastes browser clipboard text', async () => {
    readText.mockResolvedValue({ success: true, data: 'echo mobile' })
    render(<MobileTerminalControls terminalId="pty-1" />)
    fireEvent.click(screen.getByText('Paste'))
    await vi.waitFor(() => expect(write).toHaveBeenCalledWith('pty-1', 'echo mobile'))
  })
})
