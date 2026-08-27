import { describe, expect, it } from 'vitest'
import { isEditorWorkspaceCandidate, parseEditorWorkspaceList } from './editor-workspace.types'

describe('editor-workspace.types', () => {
  it('accepts a valid candidate list', () => {
    const parsed = parseEditorWorkspaceList({
      candidates: [
        {
          id: 'vscode:/users/me/app',
          editor: 'vscode',
          name: 'app',
          path: '/Users/me/app',
          source: 'recent'
        },
        {
          id: 'zed:/users/me/zed',
          editor: 'zed',
          name: 'zed',
          path: '/Users/me/zed',
          source: 'workspace-file'
        }
      ]
    })
    expect(parsed?.candidates).toHaveLength(2)
    expect(isEditorWorkspaceCandidate(parsed?.candidates[0])).toBe(true)
  })

  it('rejects unknown editors, sources, and missing fields', () => {
    expect(parseEditorWorkspaceList({ candidates: 'nope' })).toBeNull()
    expect(
      parseEditorWorkspaceList({
        candidates: [
          {
            id: 'x',
            editor: 'sublime',
            name: 'x',
            path: '/x',
            source: 'recent'
          }
        ]
      })
    ).toBeNull()
    expect(
      isEditorWorkspaceCandidate({
        id: 'x',
        editor: 'vscode',
        name: 'x',
        path: '/x',
        source: 'recent-files'
      })
    ).toBe(false)
  })
})
