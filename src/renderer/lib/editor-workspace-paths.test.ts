import { describe, expect, it } from 'vitest'
import { normalizeProjectPath } from './editor-workspace-paths'

describe('normalizeProjectPath', () => {
  it('trims slashes and unifies separators', () => {
    expect(normalizeProjectPath('/Users/foo/bar/')).toBe('/users/foo/bar')
    expect(normalizeProjectPath('C:\\Users\\foo\\bar\\')).toBe('c:/users/foo/bar')
  })

  it('keeps a bare root', () => {
    expect(normalizeProjectPath('/')).toBe('/')
  })
})
