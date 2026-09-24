import type { PendingUpdatePlan } from '@shared/types/updater.types'
import { legacyUpdateComponentPolicy } from '@shared/types/updater.types'
import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useContextBarSettingsStore } from '@/stores/context-bar-settings-store'
import { useUpdaterStore } from '@/stores/updater-store'
import type { Project } from '@/types/project'
import { DEFAULT_CONTEXT_BAR_SETTINGS } from '@/types/settings'
import { StatusBar } from './StatusBar'

// Mock the terminal store
vi.mock('@/stores/terminal-store', () => ({
  useActiveTerminal: vi.fn(() => ({
    id: 'test-terminal',
    cwd: '/home/user/project',
    gitBranch: 'feature-branch',
    gitStatus: {
      hasChanges: true,
      modified: 2,
      staged: 1,
      untracked: 3
    },
    lastExitCode: 0
  })),
  useTerminalStore: vi.fn((selector: (state: unknown) => unknown) =>
    selector({
      activeTerminalId: 'test-terminal',
      updateTerminalGitBranch: vi.fn()
    })
  )
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: vi.fn((selector: (state: unknown) => unknown) =>
    selector({
      updateProject: vi.fn()
    })
  )
}))

vi.mock('@/lib/worktree-api', () => ({
  worktreeApi: {
    branches: vi.fn(() => Promise.resolve({ success: true, data: [] }))
  }
}))

// Mock remote status store (StatusBar hosts RemoteAccessPopover)
vi.mock('@/stores/remote-status-store', () => ({
  useRemoteStatus: vi.fn(() => null),
  useRemoteRestoreError: vi.fn(() => null),
  useRemoteStatusStore: Object.assign(
    vi.fn(() => ({ setStatus: vi.fn(), setRestoreError: vi.fn() })),
    {
      getState: () => ({ status: null, setStatus: vi.fn(), setRestoreError: vi.fn() })
    }
  )
}))

vi.mock('@/lib/api', () => ({
  remoteServerApi: {
    start: vi.fn(),
    stop: vi.fn(),
    status: vi.fn(),
    intent: vi.fn(() =>
      Promise.resolve({ success: true, data: { wanted: false, publishMode: 'tunnel' } })
    ),
    setIntent: vi.fn(),
    rotateCredential: vi.fn()
  },
  openerApi: {
    openUrlWithSystemBrowser: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
  }
}))

// Mock the home directory hook
vi.mock('@/hooks/use-cwd', () => ({
  useHomeDirectory: vi.fn(() => '/home/user'),
  formatPath: vi.fn((path: string, homeDir: string) => {
    if (path.startsWith(homeDir)) {
      return `~${path.slice(homeDir.length)}`
    }
    return path
  })
}))

// Mock window.api
const mockApi = {
  persistence: {
    writeDebounced: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
  }
}

function pendingPlan(
  status: PendingUpdatePlan['status'],
  overrides: Partial<PendingUpdatePlan> = {}
): PendingUpdatePlan {
  return {
    schemaVersion: 1,
    targetVersion: '1.2.3',
    componentPolicy: legacyUpdateComponentPolicy('1.2.3'),
    status,
    currentComponentBuildIds: { terminalCore: 'live-build-not-for-ui' },
    requiredActions: ['terminalCore'],
    deferredComponents: [],
    createdAt: '2026-08-14T00:00:00.000Z',
    updatedAt: '2026-08-14T00:00:00.000Z',
    ...overrides
  }
}

beforeEach(() => {
  vi.stubGlobal('api', mockApi)
  // Reset store to defaults before each test
  useContextBarSettingsStore.setState({
    settings: { ...DEFAULT_CONTEXT_BAR_SETTINGS },
    isLoaded: true
  })
  useUpdaterStore.setState({
    pendingUpdatePlan: null,
    downloaded: false,
    version: null,
    updateAvailable: false
  })
})

afterEach(() => {
  act(() => {
    useUpdaterStore.setState({
      pendingUpdatePlan: null,
      downloaded: false,
      version: null,
      updateAvailable: false
    })
  })
  vi.unstubAllGlobals()
})

const mockProject: Project = {
  id: 'test-project',
  name: 'Test Project',
  path: '/home/user/test-project',
  color: 'blue',
  gitBranch: 'main'
}

// Helper to render with required providers
function renderWithProviders(ui: React.ReactElement) {
  return render(<TooltipProvider>{ui}</TooltipProvider>)
}

describe('StatusBar', () => {
  describe('theme chrome', () => {
    it('uses theme status-bar tokens instead of a fixed project accent', () => {
      const { container } = renderWithProviders(<StatusBar project={mockProject} />)
      const bar = container.firstElementChild

      expect(bar).toHaveClass('bg-status-bar')
      expect(bar).toHaveClass('text-status-bar-foreground')
      expect(bar).toHaveClass('border-border')
      expect(bar?.className).not.toMatch(
        /bg-(blue|purple|green|yellow|red|cyan|pink|orange|gray)-600/
      )
    })
  })

  describe('conditional rendering based on visibility settings', () => {
    it('should render git branch picker when showGitBranch is true', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.getByText('feature-branch')).toBeDefined()
      expect(screen.getByLabelText('Switch git branch')).toBeDefined()
    })

    it('should not render git branch when showGitBranch is false', () => {
      useContextBarSettingsStore.setState({
        settings: { ...DEFAULT_CONTEXT_BAR_SETTINGS, showGitBranch: false }
      })

      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.queryByText('feature-branch')).toBeNull()
    })

    it('should render git status when showGitStatus is true and has changes', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      // Check for modified count (2)
      expect(screen.getByText('2')).toBeDefined()
    })

    it('should not render git status when showGitStatus is false', () => {
      useContextBarSettingsStore.setState({
        settings: { ...DEFAULT_CONTEXT_BAR_SETTINGS, showGitStatus: false }
      })

      renderWithProviders(<StatusBar project={mockProject} />)

      // Should not find the git status numbers
      expect(screen.queryByText('2')).toBeNull()
    })

    it('should render working directory when showWorkingDirectory is true', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.getByText('~/project')).toBeDefined()
    })

    it('should not render working directory when showWorkingDirectory is false', () => {
      useContextBarSettingsStore.setState({
        settings: { ...DEFAULT_CONTEXT_BAR_SETTINGS, showWorkingDirectory: false }
      })

      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.queryByText('~/project')).toBeNull()
    })

    it('should render exit code when showExitCode is true', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.getByText('Exit: 0')).toBeDefined()
    })

    it('should not render exit code when showExitCode is false', () => {
      useContextBarSettingsStore.setState({
        settings: { ...DEFAULT_CONTEXT_BAR_SETTINGS, showExitCode: false }
      })

      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.queryByText('Exit: 0')).toBeNull()
    })

    it('should hide all optional elements when all settings are false', () => {
      useContextBarSettingsStore.setState({
        settings: {
          showGitBranch: false,
          showGitStatus: false,
          showWorkingDirectory: false,
          showExitCode: false
        }
      })

      renderWithProviders(<StatusBar project={mockProject} />)

      // Only project name should be visible
      expect(screen.getByText('test-project')).toBeDefined()
      expect(screen.queryByText('feature-branch')).toBeNull()
      expect(screen.queryByText('~/project')).toBeNull()
      expect(screen.queryByText('Exit: 0')).toBeNull()
    })
  })

  describe('project color marker', () => {
    it('uses the same 6px square micro-marker as project sidebar rows', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      const marker = document.querySelector('[data-project-color="blue"]')
      expect(marker).toHaveClass('size-1.5', 'rounded-[1px]')
      expect(marker).not.toHaveClass('size-2', 'rounded-sm')
    })
  })

  describe('project name always visible', () => {
    it('should always render project name regardless of settings', () => {
      useContextBarSettingsStore.setState({
        settings: {
          showGitBranch: false,
          showGitStatus: false,
          showWorkingDirectory: false,
          showExitCode: false
        }
      })

      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.getByText('test-project')).toBeDefined()
    })
  })

  describe('settings gear icon', () => {
    it('should render the context bar settings popover trigger', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.getByLabelText('Context bar settings')).toBeDefined()
    })
  })

  describe('remote access popover', () => {
    it('should render the remote terminal access trigger', () => {
      renderWithProviders(<StatusBar project={mockProject} />)

      expect(screen.getByLabelText('Remote terminal access')).toBeDefined()
    })

    it('should render remote trigger without an active project', () => {
      renderWithProviders(<StatusBar project={undefined} />)

      expect(screen.getByLabelText('Remote terminal access')).toBeDefined()
    })
  })

  describe('durable update plan', () => {
    it('shows reconciling, deferred, failed, and completed without guessing a live build', () => {
      useUpdaterStore.setState({
        pendingUpdatePlan: pendingPlan('reconciling')
      })
      renderWithProviders(<StatusBar project={mockProject} />)
      expect(
        screen.getByRole('status', {
          name: 'Saved update plan: components are reconnecting. This is not a zero-interruption swap.'
        })
      ).toHaveAttribute('data-update-plan-status', 'reconciling')

      act(() => {
        useUpdaterStore.setState({
          pendingUpdatePlan: pendingPlan('deferred', { deferredComponents: ['terminalCore'] })
        })
      })
      expect(
        screen.getByRole('status', {
          name: 'Saved update plan: replacement of Terminal Core is waiting because a terminal is still active. Restarting the app window does not stop that terminal.'
        })
      ).toBeInTheDocument()
      expect(screen.queryByText('live-build-not-for-ui')).not.toBeInTheDocument()

      act(() => {
        useUpdaterStore.setState({
          pendingUpdatePlan: pendingPlan('failed', { lastError: 'reconnect timed out' })
        })
      })
      expect(screen.getByRole('status')).toHaveAccessibleName(
        'Saved update plan failed. Review the error, then check again. reconnect timed out'
      )

      act(() => {
        useUpdaterStore.setState({ pendingUpdatePlan: pendingPlan('completed') })
      })
      expect(
        screen.getByRole('status', {
          name: 'Saved update plan finished. Matching Cores stayed running and were reconnected.'
        })
      ).toHaveAttribute('data-update-plan-status', 'completed')

      act(() => {
        useUpdaterStore.setState({ pendingUpdatePlan: pendingPlan('prepared') })
      })
      expect(screen.queryByRole('status')).not.toBeInTheDocument()
    })
  })
})
