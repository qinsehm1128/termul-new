import { describe, expect, it } from 'vitest'
import { buildCliSessionScopePaths } from '@/lib/cli-session-scope'

describe('buildCliSessionScopePaths', () => {
  it('omits paths for all-sessions mode', () => {
    expect(buildCliSessionScopePaths({ mode: 'all', projectPath: '/repo' })).toBeUndefined()
  })

  it('uses the directory, falling back to the project path', () => {
    expect(
      buildCliSessionScopePaths({
        mode: 'directory',
        directoryPath: '/repo/pkg',
        projectPath: '/repo'
      })
    ).toEqual(['/repo/pkg'])
    expect(
      buildCliSessionScopePaths({
        mode: 'directory',
        projectPath: '/repo'
      })
    ).toEqual(['/repo'])
  })

  it('includes project and worktree paths', () => {
    expect(
      buildCliSessionScopePaths({
        mode: 'project',
        projectPath: '/repo',
        worktreePaths: ['/repo/.worktrees/feat', '/repo']
      })
    ).toEqual(['/repo', '/repo/.worktrees/feat'])
  })
})
