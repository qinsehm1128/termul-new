import { describe, expect, it } from 'vitest'
import { findFilePathMatches } from '@/lib/file-path-links'
import { filePathFromHref, filePathHref, remarkFilePathLinks } from './chat-markdown-file-links'

describe('chat markdown file links', () => {
  it('only linkifies path-shaped tokens', () => {
    expect(findFilePathMatches('See src/App.tsx:42 and ./main.ts')).toEqual([
      { text: 'src/App.tsx:42', start: 4 },
      { text: './main.ts', start: 23 }
    ])
    expect(findFilePathMatches('https://example.com/src/App.tsx')).toEqual([])
    expect(findFilePathMatches('(src/App.tsx:42)')).toEqual([{ text: 'src/App.tsx:42', start: 1 }])
    expect(findFilePathMatches('See src/App.tsx:42,')).toEqual([
      { text: 'src/App.tsx:42', start: 4 }
    ])
    expect(findFilePathMatches('(see src/App.tsx)')).toEqual([{ text: 'src/App.tsx', start: 5 }])
  })

  it.each([
    ['scripts/release', 'scripts/release'],
    ['docs/guide.md', 'docs/guide.md'],
    ['docs/guide.md:12:3', 'docs/guide.md:12:3'],
    ['scripts/check:42', 'scripts/check:42'],
    ['docs/design%20notes.md:12:3', 'docs/design notes.md:12:3'],
    ['docs/design notes.md', 'docs/design notes.md'],
    ['./docs/guide.md', './docs/guide.md'],
    ['../scripts/release', '../scripts/release'],
    ['README.md', 'README.md']
  ])('routes explicit relative destination %s through the file marker', (url, path) => {
    const label = { type: 'text', value: 'Open docs/other.md' }
    const tree = { type: 'root', children: [{ type: 'link', url, children: [label] }] }

    remarkFilePathLinks()(tree)

    expect(tree.children).toEqual([{ type: 'link', url: filePathHref(path), children: [label] }])
    expect(filePathFromHref(tree.children[0].url)).toBe(path)
  })

  it.each([
    'https://example.com/docs/guide.md:12:3',
    'http://example.com/scripts/check',
    '//example.com/docs/guide.md',
    '#docs/guide.md',
    'docs/guide.md#section',
    'docs/guide.md?download=1',
    'mailto:hello@example.com',
    'tel:12345',
    'vscode://file/docs/guide.md',
    'javascript:alert(1)',
    'https%3A%2F%2Fexample.com%2Fdocs%2Fguide.md',
    '%23section',
    '%2F%2Fexample.com/docs/guide.md',
    'docs/bad%escape.md',
    'docs/%00private.md'
  ])('leaves external, fragment and unsafe destination %s unchanged', (url) => {
    const link = { type: 'link', url, children: [{ type: 'text', value: 'docs/label.md' }] }
    const tree = { type: 'root', children: [link] }

    remarkFilePathLinks()(tree)

    expect(tree.children).toEqual([link])
    expect(tree.children[0].url).toBe(url)
    expect(filePathFromHref(url)).toBeNull()
  })

  it('encodes and decodes path markers', () => {
    const href = filePathHref('src/renderer/App.tsx:42')
    expect(filePathFromHref(href)).toBe('src/renderer/App.tsx:42')
    expect(filePathFromHref('https://example.com')).toBeNull()
  })

  it('linkifies prose paths but leaves links and code blocks unchanged', () => {
    const tree = {
      type: 'root',
      children: [
        { type: 'paragraph', children: [{ type: 'text', value: 'See src/App.tsx:42.' }] },
        {
          type: 'link',
          url: 'https://example.com',
          children: [{ type: 'text', value: 'src/App.tsx' }]
        },
        { type: 'code', value: 'src/App.tsx:42' }
      ]
    }

    remarkFilePathLinks()(tree)

    expect(tree.children[0].children[1]).toMatchObject({
      type: 'link',
      url: filePathHref('src/App.tsx:42')
    })
    expect(tree.children[1].url).toBe('https://example.com')
    expect(tree.children[2].type).toBe('code')
  })
})
