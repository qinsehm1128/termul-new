import { render, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  resetQuickTerminalStore,
  useQuickTerminalStore
} from '@/features/quick-terminal/quick-terminal-store'
import { useProjectStore } from '@/stores/project-store'
import type { Project } from '@/types/project'
import { CliSessionPanel } from './CliSessionPanel'

const { mockListSessions } = vi.hoisted(() => ({
  mockListSessions: vi.fn(() => Promise.resolve({ sessions: [], issues: [] }))
}))

vi.mock('@/lib/api', () => ({
  cliSessionApi: { listSessions: mockListSessions, resolveSessions: vi.fn() }
}))

vi.mock('@/lib/cli-resume-defaults', () => ({
  loadCliResumeDefaults: () => Promise.resolve({ schemaVersion: 1, extraArgsByAgentId: {} })
}))

const PRIVATE_FOLDER = '/Se/terminals/2026/10/19/abc'

function renderAt(path: string): void {
  render(
    <MemoryRouter initialEntries={[path]}>
      <CliSessionPanel forceVisible />
    </MemoryRouter>
  )
}

describe('CliSessionPanel scope', () => {
  beforeEach(() => {
    mockListSessions.mockClear()
    resetQuickTerminalStore()
    useProjectStore.setState({
      projects: [{ id: 'ns-auth', name: 'ns_auth', path: '/work/ns_auth' } as Project],
      activeProjectId: 'ns-auth'
    })
    useQuickTerminalStore.setState({
      loaded: true,
      records: [
        {
          schemaVersion: 1,
          id: 'qt-1',
          target: { kind: 'workspace' },
          cwd: PRIVATE_FOLDER,
          createdAtUtc: '2026-10-19T00:00:00Z',
          updatedAtUtc: '2026-10-19T00:00:00Z',
          origin: 'created'
        }
      ]
    })
  })

  it("lists the quick terminal's own history on its page, not the active project's", async () => {
    renderAt('/quick-terminals/qt-1')
    await waitFor(() => expect(mockListSessions).toHaveBeenCalled())
    expect(mockListSessions).toHaveBeenLastCalledWith({
      scopePaths: [PRIVATE_FOLDER],
      limit: 80
    })
  })

  it('lists the active project elsewhere', async () => {
    renderAt('/')
    await waitFor(() => expect(mockListSessions).toHaveBeenCalled())
    expect(mockListSessions).toHaveBeenLastCalledWith({
      scopePaths: ['/work/ns_auth'],
      limit: 80
    })
  })
})
