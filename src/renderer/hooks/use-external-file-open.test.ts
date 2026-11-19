import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useProjectStore } from '@/stores/project-store'
import { DEFAULT_APP_SETTINGS } from '@/types/settings'
import {
  type ExternalFileHandoff,
  parseOpenFileQuery,
  useExternalFileOpen
} from './use-external-file-open'

describe('useExternalFileOpen', () => {
  beforeEach(() => {
    useAppSettingsStore.setState({ settings: DEFAULT_APP_SETTINGS, isLoaded: false })
    useProjectStore.setState({ projects: [], groups: [], activeProjectId: '', isLoaded: false })
  })

  it('parses repeated and comma-separated openFile query params', () => {
    expect(
      parseOpenFileQuery('?openFile=/a/README.md,/b/index.html&openFile=%2Fc%2Fnote.md')
    ).toEqual(['/a/README.md', '/b/index.html', '/c/note.md'])
  })

  it('waits for app settings and projects hydration before draining pending files when waitForHydration is enabled', async () => {
    let listener: ((paths: string[]) => void) | undefined
    const handoff: ExternalFileHandoff = {
      supportsExternalFileOpen: () => true,
      takePendingExternalFiles: vi.fn().mockResolvedValue(['/tmp/README.md']),
      onExternalFileOpen: vi.fn((cb) => {
        listener = cb
        return () => {
          listener = undefined
        }
      })
    }
    const route = vi.fn()

    renderHook(() =>
      useExternalFileOpen({
        handoff,
        route,
        waitForHydration: true
      })
    )

    expect(handoff.takePendingExternalFiles).not.toHaveBeenCalled()
    expect(route).not.toHaveBeenCalled()

    act(() => {
      useAppSettingsStore.setState({ isLoaded: true })
      useProjectStore.setState({ isLoaded: true })
    })

    await waitFor(() => {
      expect(handoff.takePendingExternalFiles).toHaveBeenCalledTimes(1)
      expect(route).toHaveBeenCalledWith(['/tmp/README.md'])
    })

    act(() => {
      listener?.(['/tmp/live.html'])
    })
    expect(route).toHaveBeenCalledWith(['/tmp/live.html'])
  })
})
