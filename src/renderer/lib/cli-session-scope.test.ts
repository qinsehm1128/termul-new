import type { QuickTerminalRecord, QuickTerminalTarget } from '@shared/types/quick-terminal.types'
import { describe, expect, it } from 'vitest'
import {
  buildCliSessionScopePaths,
  quickTerminalCliSessionScope,
  resolveCliSessionDirectory
} from '@/lib/cli-session-scope'

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

describe('quickTerminalCliSessionScope', () => {
  function record(target: QuickTerminalTarget, cwd: string): QuickTerminalRecord {
    return {
      schemaVersion: 1,
      id: 'qt-1',
      target,
      cwd,
      createdAtUtc: '2026-10-19T00:00:00Z',
      updatedAtUtc: '2026-10-19T00:00:00Z',
      origin: 'created'
    }
  }
  const projects = [
    { id: 'other', path: '/work/other' },
    { id: 'repo', path: '/work/repo', worktrees: [{ path: '/work/repo/.worktrees/feat' }] }
  ] as Parameters<typeof quickTerminalCliSessionScope>[1]

  it('scopes a private folder to that folder, not to the project open before', () => {
    // The defect this replaced: the panel listed the previously active
    // project's history on a quick terminal's page.
    expect(
      quickTerminalCliSessionScope(
        record({ kind: 'workspace' }, '/Se/terminals/2026/10/19/abc'),
        projects
      )
    ).toEqual({
      directoryPath: '/Se/terminals/2026/10/19/abc',
      projectPath: '/Se/terminals/2026/10/19/abc',
      worktreePaths: []
    })
  })

  it('scopes a project shell to the project it was opened in', () => {
    expect(
      quickTerminalCliSessionScope(
        record({ kind: 'project_root', projectId: 'repo', projectRoot: '/stale' }, '/work/repo'),
        projects
      )
    ).toEqual({
      directoryPath: '/work/repo',
      projectPath: '/work/repo',
      worktreePaths: ['/work/repo/.worktrees/feat']
    })
  })

  it('falls back to the recorded path when the project is gone', () => {
    expect(
      quickTerminalCliSessionScope(
        record(
          {
            kind: 'worktree',
            projectId: 'removed',
            worktreePath: '/gone/wt',
            worktreeBranch: 'feat'
          },
          '/gone/wt'
        ),
        projects
      ).projectPath
    ).toBe('/gone/wt')
  })
})
