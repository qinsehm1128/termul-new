import { describe, expect, it } from 'vitest'
import {
  applyTerminalRenderOptions,
  DEFAULT_TERMINAL_OPTIONS,
  getTerminalOptions
} from './terminal-config'

describe('getTerminalOptions', () => {
  it('should return windowsPty config for Windows platform', () => {
    const options = getTerminalOptions('Win32')
    expect(options.windowsPty).toEqual({
      backend: 'conpty',
      buildNumber: 21376
    })
    expect(options.convertEol).toBe(false)
    expect(options.ignoreBracketedPasteMode).toBe(false)
  })

  it('should not include windowsPty for macOS platform', () => {
    const options = getTerminalOptions('MacIntel')
    expect(options.windowsPty).toBeUndefined()
    expect(options.convertEol).toBe(false)
    expect(options.ignoreBracketedPasteMode).toBe(false)
  })

  it('should not include windowsPty for Linux platform', () => {
    const options = getTerminalOptions('Linux x86_64')
    expect(options.windowsPty).toBeUndefined()
    expect(options.convertEol).toBe(false)
    expect(options.ignoreBracketedPasteMode).toBe(false)
  })

  it('should include base terminal options for all platforms', () => {
    const windowsOptions = getTerminalOptions('Win32')
    const macOptions = getTerminalOptions('MacIntel')

    // Both should have base options
    expect(windowsOptions.cursorBlink).toBe(true)
    expect(windowsOptions.cursorStyle).toBe('block')
    expect(macOptions.cursorBlink).toBe(true)
    expect(macOptions.cursorStyle).toBe('block')
  })
})

describe('DEFAULT_TERMINAL_OPTIONS', () => {
  it('should have convertEol set to false', () => {
    expect(DEFAULT_TERMINAL_OPTIONS.convertEol).toBe(false)
    expect(DEFAULT_TERMINAL_OPTIONS.ignoreBracketedPasteMode).toBe(false)
  })

  it('should have expected default values', () => {
    expect(DEFAULT_TERMINAL_OPTIONS.fontSize).toBe(14)
    expect(DEFAULT_TERMINAL_OPTIONS.lineHeight).toBe(1)
    expect(DEFAULT_TERMINAL_OPTIONS.letterSpacing).toBe(0)
    expect(DEFAULT_TERMINAL_OPTIONS.smoothScrollDuration).toBe(0)
    expect(DEFAULT_TERMINAL_OPTIONS.rescaleOverlappingGlyphs).toBe(true)
    expect(DEFAULT_TERMINAL_OPTIONS.scrollback).toBe(10000)
    expect(DEFAULT_TERMINAL_OPTIONS.cursorBlink).toBe(true)
    expect(DEFAULT_TERMINAL_OPTIONS.drawBoldTextInBrightColors).toBe(true)
    expect(DEFAULT_TERMINAL_OPTIONS.scrollbar).toEqual({ showScrollbar: false })
    expect(DEFAULT_TERMINAL_OPTIONS.allowProposedApi).toBe(true)
  })

  it('applies cell-metric options onto a cached terminal', () => {
    const terminal = {
      options: {
        lineHeight: 1.2,
        letterSpacing: 2,
        smoothScrollDuration: 150,
        rescaleOverlappingGlyphs: false,
        scrollbar: { showScrollbar: true, width: 12 }
      }
    }
    applyTerminalRenderOptions(terminal)
    expect(terminal.options.lineHeight).toBe(1)
    expect(terminal.options.letterSpacing).toBe(0)
    expect(terminal.options.smoothScrollDuration).toBe(0)
    expect(terminal.options.rescaleOverlappingGlyphs).toBe(true)
    expect(terminal.options.scrollbar).toEqual({ showScrollbar: false })
  })

  it('should disable screenReaderMode to avoid duplicate PTY input (#267)', () => {
    expect(DEFAULT_TERMINAL_OPTIONS.screenReaderMode).toBe(false)
  })
})
