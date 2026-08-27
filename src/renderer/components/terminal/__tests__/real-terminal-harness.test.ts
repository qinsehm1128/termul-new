import { afterEach, describe, expect, it } from 'vitest'
import {
  createRealTerminalHarness,
  createXtermScreenFixture,
  type RealTerminalHarness,
  type XtermScreenFixture
} from './real-terminal-harness'

describe('createRealTerminalHarness', () => {
  let harness: RealTerminalHarness | null = null

  afterEach(() => {
    harness?.dispose()
    harness = null
  })

  it('reports the pre-Unicode11 baseline on a real constructed Terminal', () => {
    harness = createRealTerminalHarness({ allowProposedApi: true })

    expect(harness.readActiveUnicodeVersion()).toBe('6')
    expect(harness.terminal.unicode.versions).toEqual(['6'])
  })

  it('throws on the unicode axis when allowProposedApi is not set', () => {
    harness = createRealTerminalHarness()

    expect(() => harness?.readActiveUnicodeVersion()).toThrow(
      'You must set the allowProposedApi option to true to use proposed API'
    )
  })
})

describe('createXtermScreenFixture', () => {
  let fixture: XtermScreenFixture | null = null
  let injectedStyle: HTMLStyleElement | null = null

  afterEach(() => {
    fixture?.dispose()
    fixture = null
    injectedStyle?.remove()
    injectedStyle = null
  })

  it('tracks the inline will-change lifecycle through computed style', () => {
    fixture = createXtermScreenFixture()

    expect(fixture.readComputedWillChange()).toBe('')

    fixture.screen.style.willChange = 'transform'
    expect(fixture.readComputedWillChange()).toBe('transform')

    fixture.screen.style.willChange = ''
    expect(fixture.readComputedWillChange()).toBe('')
  })

  it('observes a promotion coming from the CSS cascade, not only inline style', () => {
    injectedStyle = document.createElement('style')
    injectedStyle.textContent = '.xterm-screen { will-change: transform }'
    document.head.appendChild(injectedStyle)

    fixture = createXtermScreenFixture()

    expect(fixture.readInlineWillChange()).toBe('')
    expect(fixture.readComputedWillChange()).toBe('transform')
  })
})
