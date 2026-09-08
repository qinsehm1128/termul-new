import { describe, expect, it } from 'vitest'
import { buildCliSessionScopePaths, resolveCliSessionDirectory } from '@/lib/cli-session-scope'

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

describe('resolveCliSessionDirectory', () => {
  it('prefers the open conversation over the project', () => {
    // The defect this replaced: inside a Conversation the panel listed the
    // project's history, not the folder on screen.
    expect(
      resolveCliSessionDirectory({
        conversationWorkspaceCwd: '/sessions/2026/09/08/abc',
        terminalCwd: '/work/repo',
        projectDefaultCwd: '/work/repo'
      })
    ).toBe('/sessions/2026/09/08/abc')
  })

  it('falls back to the focused terminal when no conversation is open', () => {
    expect(
      resolveCliSessionDirectory({
        conversationWorkspaceCwd: null,
        terminalCwd: '/work/sub',
        projectDefaultCwd: '/work'
      })
    ).toBe('/work/sub')
  })

  it('falls back to the project default last', () => {
    expect(
      resolveCliSessionDirectory({ conversationWorkspaceCwd: null, projectDefaultCwd: '/work' })
    ).toBe('/work')
  })

  it('treats an empty string as absent rather than as the process cwd', () => {
    // An empty cwd would scope the scan to nothing useful while looking valid.
    expect(
      resolveCliSessionDirectory({ conversationWorkspaceCwd: '', projectDefaultCwd: '/work' })
    ).toBe('/work')
  })

  it('returns null when nothing is resolvable', () => {
    expect(resolveCliSessionDirectory({})).toBeNull()
  })
})
