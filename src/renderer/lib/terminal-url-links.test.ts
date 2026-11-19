import { describe, expect, it, vi } from 'vitest'
import {
  buildTerminalUrlLinks,
  isSupportedTerminalUrl,
  isTerminalUrlCandidate,
  sanitizeTerminalUrlToken
} from './terminal-url-links'

const links = (line: string) =>
  buildTerminalUrlLinks(line, 1, vi.fn()).map(({ text, range }) => ({ text, range }))

describe('terminal-url-links', () => {
  it('extracts a plain http(s) URL with its real cells', () => {
    expect(links('open https://example.com/a?b=1#c now')).toEqual([
      {
        text: 'https://example.com/a?b=1#c',
        range: { start: { x: 6, y: 1 }, end: { x: 5 + 'https://example.com/a?b=1#c'.length, y: 1 } }
      }
    ])
  })

  it('keeps the start index on the URL when a wrapper precedes it', () => {
    const url = 'https://example.com/path'
    const wrapped = `(${url})`
    expect(links(wrapped)).toEqual([
      {
        text: url,
        range: { start: { x: 2, y: 1 }, end: { x: 1 + url.length, y: 1 } }
      }
    ])
  })

  it('does not shift the range when sanitizing trims a leading wrapper', () => {
    const url = 'https://example.com'
    const raw = `(${url}`
    // A raw candidate that still carries a leading wrapper must anchor on the URL.
    expect(sanitizeTerminalUrlToken(raw)).toBe(url)
    expect(links(`x${raw}`)).toEqual([
      {
        text: url,
        range: { start: { x: 3, y: 1 }, end: { x: 2 + url.length, y: 1 } }
      }
    ])
  })

  it.each([
    ['「https://example.com」', 'https://example.com'],
    ['【https://example.com】', 'https://example.com'],
    ['“https://example.com”', 'https://example.com'],
    ['（https://example.com）', 'https://example.com']
  ])('trims CJK wrappers: %s', (line, expected) => {
    expect(links(line).map((link) => link.text)).toEqual([expected])
  })

  it.each([
    ['see https://example.com。', 'https://example.com'],
    ['see https://example.com，然后', 'https://example.com'],
    ['see https://example.com、docs', 'https://example.com'],
    ['see https://example.com) next', 'https://example.com'],
    ['see https://example.com, next', 'https://example.com']
  ])('stops at trailing punctuation: %s', (line, expected) => {
    expect(links(line).map((link) => link.text)).toEqual([expected])
  })

  it('finds a URL embedded in url= assignment', () => {
    expect(links('url=https://example.com/x').map((link) => link.text)).toEqual([
      'https://example.com/x'
    ])
  })

  it('reports unsupported schemes as candidates but not supported', () => {
    expect(links('mailto:hello@example.com').map((link) => link.text)).toEqual([
      'mailto:hello@example.com'
    ])
    expect(isTerminalUrlCandidate('mailto:hello@example.com')).toBe(true)
    expect(isSupportedTerminalUrl('mailto:hello@example.com')).toBe(false)
    expect(isSupportedTerminalUrl('https://example.com')).toBe(true)
  })

  it('never treats a Windows drive path as a URL candidate', () => {
    expect(isTerminalUrlCandidate('C:/repo/src/App.tsx')).toBe(false)
  })
})
