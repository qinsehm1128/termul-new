import { describe, expect, it } from 'vitest'
import {
  extractMarkdownImageReferences,
  resolveImageFileUrl,
  restoreMarkdownImageUrls
} from './use-blocknote'

describe('Markdown image URLs', () => {
  it('extracts Markdown and HTML image references in document order', () => {
    expect(
      extractMarkdownImageReferences(
        [
          '![first](./images/first.png)',
          '<img alt="second" src="../images/second.jpg">',
          '![third](https://example.com/third.webp)'
        ].join('\n')
      )
    ).toEqual(['./images/first.png', '../images/second.jpg', 'https://example.com/third.webp'])
  })

  it('restores original URLs without changing other image props', () => {
    const blocks = [
      {
        type: 'image',
        props: { url: 'http://localhost/absolutized.png', name: 'first', previewWidth: 320 }
      },
      { type: 'paragraph', content: [] },
      { type: 'image', props: { url: 'http://localhost/second.png', name: 'second' } }
    ]

    expect(restoreMarkdownImageUrls(blocks, ['./images/first.png', '../second.png'])).toEqual([
      {
        type: 'image',
        props: { url: './images/first.png', name: 'first', previewWidth: 320 }
      },
      { type: 'paragraph', content: [] },
      { type: 'image', props: { url: '../second.png', name: 'second' } }
    ])
  })

  it('does not rewrite external URLs in the web runtime', async () => {
    await expect(
      resolveImageFileUrl('https://example.com/image.png', '/workspace/readme.md')
    ).resolves.toBe('https://example.com/image.png')
    await expect(resolveImageFileUrl('./image.png', '/workspace/readme.md')).resolves.toBe(
      './image.png'
    )
  })
})
