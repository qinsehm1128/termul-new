import { beforeEach, describe, expect, it, vi } from 'vitest'
import { terminalApi } from '@/lib/terminal-api'
import { serializeTerminalsForProject } from '../hooks/useTerminalAutoSave'

vi.mock('@/lib/terminal-api', () => ({
  terminalApi: {
    closeView: vi.fn(async () => ({ success: true, data: undefined })),
    terminate: vi.fn(async () => ({ success: true, data: undefined })),
    spawn: vi.fn(),
    resume: vi.fn()
  }
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn(async () => undefined)
}))

import { logFrontendError } from '@/lib/log-api'

import { useProjectStore } from './project-store'
import { useSessionWorkspaceSyncStore } from './session-workspace-sync-store'
import {
  HIDDEN_BUFFER_TRUNCATION_DELAY,
  MAX_TRANSCRIPT_CHARS,
  TRUNCATED_BUFFER_SIZE,
  useTerminalStore
} from './terminal-store'

describe('terminal-store', () => {
  beforeEach(() => {
    // Reset stores to initial state before each test
    useSessionWorkspaceSyncStore.setState({
      activeConversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    })
    useProjectStore.setState({
      projects: [
        { id: '1', name: 'Project 1', color: 'blue', isActive: true },
        { id: '2', name: 'Project 2', color: 'green' }
      ],
      activeProjectId: '1'
    })

    useTerminalStore.setState({
      terminals: [
        { id: 't1', name: 'Terminal 1', projectId: '1', shell: 'powershell', output: [] },
        { id: 't2', name: 'Terminal 2', projectId: '1', shell: 'powershell', output: [] },
        { id: 't3', name: 'Terminal 3', projectId: '2', shell: 'bash', output: [] }
      ],
      activeTerminalId: 't1',
      recentTerminalIds: [],
      ptyIdIndex: new Map(),
      cleanupRecoveries: {}
    })
    vi.mocked(terminalApi.closeView).mockReset()
    vi.mocked(terminalApi.closeView).mockResolvedValue({ success: true, data: undefined })
    vi.mocked(terminalApi.terminate).mockReset()
    vi.mocked(terminalApi.terminate).mockResolvedValue({ success: true, data: undefined })
    vi.mocked(terminalApi.spawn).mockReset()
    vi.mocked(terminalApi.resume).mockReset()
    vi.mocked(logFrontendError).mockClear()
  })

  describe('initial state', () => {
    it('should have empty terminals array by default', () => {
      // Reset to true initial state (no beforeEach data)
      useTerminalStore.setState({ terminals: [], activeTerminalId: '' })
      const { terminals } = useTerminalStore.getState()
      expect(terminals).toEqual([])
    })

    it('should have empty activeTerminalId by default', () => {
      // Reset to true initial state (no beforeEach data)
      useTerminalStore.setState({ terminals: [], activeTerminalId: '' })
      const { activeTerminalId } = useTerminalStore.getState()
      expect(activeTerminalId).toBe('')
    })
  })

  describe('selectTerminal', () => {
    it('should update activeTerminalId', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t2')

      const { activeTerminalId } = useTerminalStore.getState()
      expect(activeTerminalId).toBe('t2')
    })

    it('should update isActive property on terminals', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t2')

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      const terminal2 = terminals.find((t) => t.id === 't2')

      expect(terminal1?.isActive).toBe(false)
      expect(terminal2?.isActive).toBe(true)
    })
  })

  describe('getPreviousTerminalId', () => {
    it('should return the terminal you came from', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t1')
      selectTerminal('t2')

      expect(useTerminalStore.getState().getPreviousTerminalId()).toBe('t1')
    })

    it('should not return the active terminal when it is reselected', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t1')
      selectTerminal('t2')
      // Re-selecting the terminal you are already on must not turn it into its
      // own "previous" — that would make the jump a no-op.
      selectTerminal('t2')

      expect(useTerminalStore.getState().getPreviousTerminalId()).toBe('t1')
    })

    it('should skip terminals that no longer exist', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t1')
      selectTerminal('t2')
      selectTerminal('t3')
      // t2 is the freshest non-active entry, but it is gone. The answer must
      // fall through to t1 rather than hand back a dead id.
      useTerminalStore.getState().closeTerminal('t2', '1')
      useTerminalStore.setState({ activeTerminalId: 't3' })

      expect(useTerminalStore.getState().getPreviousTerminalId()).toBe('t1')
    })

    it('should return undefined when nothing else has been visited', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t1')

      expect(useTerminalStore.getState().getPreviousTerminalId()).toBeUndefined()
    })

    it('should keep the recent stack deduplicated so older history survives', () => {
      const { selectTerminal } = useTerminalStore.getState()
      selectTerminal('t3')
      // Ping-ponging between two terminals must not crowd t3 out of a bounded
      // stack — the quick switcher orders by this list, not just its head.
      for (let round = 0; round < 5; round += 1) {
        selectTerminal('t1')
        selectTerminal('t2')
      }

      expect(useTerminalStore.getState().recentTerminalIds).toEqual(['t2', 't1', 't3'])
    })

    it('should record terminals that become active without an explicit select', () => {
      // A freshly created terminal is activated inside addTerminal, not through
      // selectTerminal. Missing that transition would strand the jump on the
      // terminal the user just left behind.
      const { addTerminal } = useTerminalStore.getState()
      const created = addTerminal('Terminal 4', '1', 'bash')
      useTerminalStore.getState().selectTerminal('t1')

      expect(useTerminalStore.getState().getPreviousTerminalId()).toBe(created.id)
    })
  })

  describe('addTerminal', () => {
    it('should add a new terminal to the array', () => {
      const { addTerminal } = useTerminalStore.getState()
      const initialCount = useTerminalStore.getState().terminals.length

      addTerminal('New Terminal', '1')

      const { terminals } = useTerminalStore.getState()
      expect(terminals.length).toBe(initialCount + 1)
    })

    it('should return the created terminal', () => {
      const { addTerminal } = useTerminalStore.getState()
      const newTerminal = addTerminal('Test Terminal', '1', 'bash')

      expect(newTerminal.name).toBe('Test Terminal')
      expect(newTerminal.projectId).toBe('1')
      expect(newTerminal.shell).toBe('bash')
      expect(newTerminal.id).toBeTruthy()
    })

    it('should set activeTerminalId to new terminal', () => {
      const { addTerminal } = useTerminalStore.getState()
      const newTerminal = addTerminal('New', '1')

      const { activeTerminalId } = useTerminalStore.getState()
      expect(activeTerminalId).toBe(newTerminal.id)
    })

    it('should default shell to powershell', () => {
      const { addTerminal } = useTerminalStore.getState()
      const newTerminal = addTerminal('Test', '1')

      expect(newTerminal.shell).toBe('powershell')
    })

    it('should store cwd when provided', () => {
      const { addTerminal } = useTerminalStore.getState()
      const newTerminal = addTerminal('Test', '1', 'bash', '/home/user/project')

      expect(newTerminal.cwd).toBe('/home/user/project')
    })

    it('should have undefined cwd when not provided', () => {
      const { addTerminal } = useTerminalStore.getState()
      const newTerminal = addTerminal('Test', '1', 'powershell')

      expect(newTerminal.cwd).toBeUndefined()
    })
  })

  describe('resumeTerminalResource', () => {
    it('treats a scope-less running project terminal with a claim as already attached', async () => {
      useTerminalStore.setState({
        terminals: [
          {
            id: 'proj-term',
            name: 'Project terminal',
            projectId: '1',
            shell: 'bash',
            ptyId: 'pty-proj',
            claim: 'spawn-claim',
            healthStatus: 'running',
            output: []
          }
        ],
        activeTerminalId: 'proj-term',
        ptyIdIndex: new Map([['pty-proj', 'proj-term']]),
        cleanupRecoveries: {}
      })

      const result = await useTerminalStore.getState().resumeTerminalResource('proj-term')

      // `null`, not a coverage object: nothing was replayed, so the renderer
      // transcript stays the only source of continuity for this record.
      expect(result).toEqual({ success: true, data: null })
      expect(terminalApi.resume).not.toHaveBeenCalled()
    })

    it('does not call the Conversation resume path for a scope-less terminal without a claim', async () => {
      useTerminalStore.setState({
        terminals: [
          {
            id: 'proj-term',
            name: 'Project terminal',
            projectId: '1',
            shell: 'bash',
            ptyId: 'pty-proj',
            healthStatus: 'running',
            output: []
          }
        ],
        activeTerminalId: 'proj-term',
        ptyIdIndex: new Map([['pty-proj', 'proj-term']]),
        cleanupRecoveries: {}
      })

      const result = await useTerminalStore.getState().resumeTerminalResource('proj-term')

      expect(result).toEqual({
        success: false,
        error: 'Terminal unavailable',
        code: 'TERMINAL_NOT_FOUND'
      })
      expect(terminalApi.resume).not.toHaveBeenCalled()
    })
  })

  describe('restartTerminalResource', () => {
    it('respawns a scope-less project terminal without a Conversation id', async () => {
      vi.mocked(terminalApi.spawn).mockResolvedValue({
        success: true,
        data: {
          id: 'pty-restarted-project',
          shell: 'bash',
          cwd: '/tmp',
          pid: 3,
          cols: 80,
          rows: 24,
          claim: 'fresh-project-claim'
        }
      })
      useTerminalStore.setState({
        terminals: [
          {
            id: 'proj-term',
            name: 'Project terminal',
            projectId: '1',
            shell: 'bash',
            ptyId: 'pty-proj',
            claim: 'old-claim',
            healthStatus: 'running',
            output: []
          }
        ],
        activeTerminalId: 'proj-term',
        ptyIdIndex: new Map([['pty-proj', 'proj-term']]),
        cleanupRecoveries: {}
      })

      const restarted = await useTerminalStore.getState().restartTerminalResource('proj-term')

      expect(restarted).toBe(true)
      expect(terminalApi.terminate).toHaveBeenCalledWith('pty-proj')
      expect(terminalApi.spawn).toHaveBeenCalledWith(
        expect.objectContaining({
          projectId: '1',
          shell: 'bash'
        })
      )
      expect(vi.mocked(terminalApi.spawn).mock.calls[0]?.[0]).not.toHaveProperty('conversationId')
      expect(useTerminalStore.getState().terminals[0]).toMatchObject({
        id: 'proj-term',
        ptyId: 'pty-restarted-project',
        claim: 'fresh-project-claim',
        healthStatus: 'running'
      })
    })
  })

  describe('closeTerminal', () => {
    it('should remove terminal from array', () => {
      const { closeTerminal } = useTerminalStore.getState()
      const initialCount = useTerminalStore.getState().terminals.length

      closeTerminal('t2', '1')

      const { terminals } = useTerminalStore.getState()
      expect(terminals.length).toBe(initialCount - 1)
      expect(terminals.find((t) => t.id === 't2')).toBeUndefined()
    })

    it('should update activeTerminalId when closing active terminal', () => {
      const { closeTerminal } = useTerminalStore.getState()
      closeTerminal('t1', '1')

      const { activeTerminalId } = useTerminalStore.getState()
      expect(activeTerminalId).toBe('t2')
    })

    it('should not change activeTerminalId when closing non-active terminal', () => {
      const { closeTerminal } = useTerminalStore.getState()
      closeTerminal('t2', '1')

      const { activeTerminalId } = useTerminalStore.getState()
      expect(activeTerminalId).toBe('t1')
    })

    it('hides the view even when closeView IPC fails', async () => {
      useTerminalStore.setState((state) => ({
        terminals: state.terminals.map((terminal) =>
          terminal.id === 't1' ? { ...terminal, ptyId: 'pty-1' } : terminal
        )
      }))
      vi.mocked(terminalApi.closeView).mockResolvedValueOnce({
        success: false,
        error: 'forwarder busy',
        code: 'INVOKE_ERROR'
      })
      const { closeTerminalView } = useTerminalStore.getState()

      await expect(closeTerminalView('t1')).resolves.toBe(true)
      expect(terminalApi.closeView).toHaveBeenCalledWith('pty-1')

      expect(useTerminalStore.getState().terminals.find((t) => t.id === 't1')).toMatchObject({
        viewState: 'hidden',
        isHidden: true
      })
    })

    it('should set empty activeTerminalId when closing last terminal for project', () => {
      // Close all terminals for project 2
      const { closeTerminal } = useTerminalStore.getState()
      useTerminalStore.setState({ activeTerminalId: 't3' })

      closeTerminal('t3', '2')

      const { activeTerminalId } = useTerminalStore.getState()
      expect(activeTerminalId).toBe('')
    })
  })

  describe('renameTerminal', () => {
    it('should update terminal name', () => {
      const { renameTerminal } = useTerminalStore.getState()
      renameTerminal('t1', 'Renamed Terminal')

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')

      expect(terminal?.name).toBe('Renamed Terminal')
    })
  })

  describe('reorderTerminals', () => {
    it('should reorder terminals for a project', () => {
      const { reorderTerminals } = useTerminalStore.getState()
      reorderTerminals('1', ['t2', 't1'])

      const { terminals } = useTerminalStore.getState()
      const projectTerminals = terminals.filter((t) => t.projectId === '1')

      expect(projectTerminals[0].id).toBe('t2')
      expect(projectTerminals[1].id).toBe('t1')
    })

    it('should not affect terminals from other projects', () => {
      const { reorderTerminals } = useTerminalStore.getState()
      reorderTerminals('1', ['t2', 't1'])

      const { terminals } = useTerminalStore.getState()
      const project2Terminals = terminals.filter((t) => t.projectId === '2')

      expect(project2Terminals.length).toBe(1)
      expect(project2Terminals[0].id).toBe('t3')
    })
  })

  describe('setTerminals', () => {
    it('should replace all terminals', () => {
      const { setTerminals } = useTerminalStore.getState()
      const newTerminals = [
        { id: 'new', name: 'New', projectId: '1', shell: 'bash' as const, output: [] }
      ]

      setTerminals(newTerminals)

      const { terminals } = useTerminalStore.getState()
      expect(terminals.length).toBe(1)
      expect(terminals[0].id).toBe('new')
    })
  })

  describe('setTerminalPtyId', () => {
    it('should set ptyId on existing terminal', () => {
      const { setTerminalPtyId } = useTerminalStore.getState()

      const didSet = setTerminalPtyId('t1', 'terminal-123-1')

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(didSet).toBe(true)
      expect(terminal?.ptyId).toBe('terminal-123-1')
    })

    it('should not affect other terminals', () => {
      const { setTerminalPtyId } = useTerminalStore.getState()

      const didSet = setTerminalPtyId('t1', 'terminal-123-1')

      const { terminals } = useTerminalStore.getState()
      const terminal2 = terminals.find((t) => t.id === 't2')
      expect(didSet).toBe(true)
      expect(terminal2?.ptyId).toBeUndefined()
    })

    it('should reject assigning same ptyId to different terminal', () => {
      const { setTerminalPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'terminal-shared')
      const secondSet = setTerminalPtyId('t2', 'terminal-shared')

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      const terminal2 = terminals.find((t) => t.id === 't2')

      expect(secondSet).toBe(false)
      expect(terminal1?.ptyId).toBe('terminal-shared')
      expect(terminal2?.ptyId).toBeUndefined()
    })

    it('should ignore attempts to replace existing different ptyId', () => {
      const { setTerminalPtyId } = useTerminalStore.getState()

      const firstSet = setTerminalPtyId('t1', 'terminal-old')
      const secondSet = setTerminalPtyId('t1', 'terminal-new')

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(firstSet).toBe(true)
      expect(secondSet).toBe(false)
      expect(terminal?.ptyId).toBe('terminal-old')
    })
  })

  describe('findTerminalByPtyId', () => {
    it('should find terminal by ptyId', () => {
      const { setTerminalPtyId, findTerminalByPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'terminal-123-1')
      const terminal = findTerminalByPtyId('terminal-123-1')

      expect(terminal).toBeDefined()
      expect(terminal?.id).toBe('t1')
    })

    it('should return undefined when ptyId not found', () => {
      const { findTerminalByPtyId } = useTerminalStore.getState()

      const terminal = findTerminalByPtyId('non-existent')
      expect(terminal).toBeUndefined()
    })

    it('should find correct terminal when multiple have ptyIds', () => {
      const { setTerminalPtyId, findTerminalByPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'terminal-123-1')
      setTerminalPtyId('t2', 'terminal-123-2')

      const terminal1 = findTerminalByPtyId('terminal-123-1')
      const terminal2 = findTerminalByPtyId('terminal-123-2')

      expect(terminal1?.id).toBe('t1')
      expect(terminal2?.id).toBe('t2')
    })
  })

  describe('setTerminalResumeCursor', () => {
    const cursorOf = (id: string): number | undefined =>
      useTerminalStore.getState().terminals.find((t) => t.id === id)?.resumeCursor

    it('should record the cursor an attach returned', () => {
      useTerminalStore.getState().setTerminalResumeCursor('t1', 87)

      expect(cursorOf('t1')).toBe(87)
    })

    it('should never rewind the cursor', () => {
      const { setTerminalResumeCursor } = useTerminalStore.getState()

      setTerminalResumeCursor('t1', 87)
      // A resume and a watch can settle in either order; taking the smaller
      // value would widen the next replay back over already-delivered bytes.
      setTerminalResumeCursor('t1', 12)

      expect(cursorOf('t1')).toBe(87)
    })

    it('should ignore values that cannot be a cursor', () => {
      const { setTerminalResumeCursor } = useTerminalStore.getState()

      setTerminalResumeCursor('t1', 87)
      setTerminalResumeCursor('t1', -1)
      setTerminalResumeCursor('t1', Number.NaN)

      expect(cursorOf('t1')).toBe(87)
    })

    it('should not touch other terminals', () => {
      useTerminalStore.getState().setTerminalResumeCursor('t1', 87)

      expect(cursorOf('t2')).toBeUndefined()
    })
  })

  describe('clearTerminalPtyId', () => {
    it('should clear ptyId from matching terminal', () => {
      const { setTerminalPtyId, clearTerminalPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'terminal-123-1')
      clearTerminalPtyId('terminal-123-1')

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(terminal?.ptyId).toBeUndefined()
    })

    it('should not affect terminals with different ptyId', () => {
      const { setTerminalPtyId, clearTerminalPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'terminal-123-1')
      setTerminalPtyId('t2', 'terminal-123-2')
      clearTerminalPtyId('terminal-123-1')

      const { terminals } = useTerminalStore.getState()
      const terminal2 = terminals.find((t) => t.id === 't2')
      expect(terminal2?.ptyId).toBe('terminal-123-2')
    })

    it('should be a no-op when ptyId does not exist', () => {
      const { setTerminalPtyId, clearTerminalPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'terminal-123-1')
      clearTerminalPtyId('non-existent')

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      expect(terminal1?.ptyId).toBe('terminal-123-1')
    })
  })

  describe('updateTerminalExitCode', () => {
    it('should update exit code for existing terminal', () => {
      const { updateTerminalExitCode } = useTerminalStore.getState()

      updateTerminalExitCode('t1', 0)

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(terminal?.lastExitCode).toBe(0)
    })

    it('should update exit code to non-zero value', () => {
      const { updateTerminalExitCode } = useTerminalStore.getState()

      updateTerminalExitCode('t1', 127)

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(terminal?.lastExitCode).toBe(127)
    })

    it('should update exit code to null', () => {
      const { updateTerminalExitCode } = useTerminalStore.getState()

      // First set a value
      updateTerminalExitCode('t1', 1)
      expect(useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.lastExitCode).toBe(1)

      // Then reset to null
      updateTerminalExitCode('t1', null)
      expect(
        useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.lastExitCode
      ).toBeNull()
    })

    it('should not affect other terminals', () => {
      const { updateTerminalExitCode } = useTerminalStore.getState()

      updateTerminalExitCode('t1', 42)

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      const terminal2 = terminals.find((t) => t.id === 't2')

      expect(terminal1?.lastExitCode).toBe(42)
      expect(terminal2?.lastExitCode).toBeUndefined()
    })
  })

  describe('detached output buffering', () => {
    it('should append detached output for terminal by ptyId', () => {
      const { setTerminalPtyId, appendDetachedOutput } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-detached-1')

      appendDetachedOutput('pty-detached-1', 'hello')
      appendDetachedOutput('pty-detached-1', ' world')

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.detachedOutput).toBe('hello world')
    })

    it('should consume detached output once and clear it', () => {
      const { setTerminalPtyId, appendDetachedOutput, consumeDetachedOutput } =
        useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-detached-1')
      appendDetachedOutput('pty-detached-1', 'stream chunk')

      expect(consumeDetachedOutput('pty-detached-1')).toBe('stream chunk')
      expect(consumeDetachedOutput('pty-detached-1')).toBe('')
    })

    it('should track renderer attachment count per pty', () => {
      const { setTerminalPtyId, setRendererAttached } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-renderer-1')

      setRendererAttached('pty-renderer-1', true)
      setRendererAttached('pty-renderer-1', true)
      setRendererAttached('pty-renderer-1', false)

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.rendererAttachmentCount).toBe(1)
    })

    it('warns without throwing when setRendererAttached finds no terminal for the ptyId', () => {
      expect(() =>
        useTerminalStore.getState().setRendererAttached('pty-unknown', true)
      ).not.toThrow()

      expect(logFrontendError).toHaveBeenCalledWith(
        expect.objectContaining({
          level: 'warn',
          source: 'terminal-store.setRendererAttached'
        })
      )
      // R-11: the miss stays a no-op; only its visibility changed.
      expect(
        useTerminalStore.getState().terminals.every((t) => t.rendererAttachmentCount === undefined)
      ).toBe(true)
    })

    it('does not warn on a successful setRendererAttached', () => {
      const { setTerminalPtyId, setRendererAttached } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-renderer-ok')

      setRendererAttached('pty-renderer-ok', true)

      expect(logFrontendError).not.toHaveBeenCalled()
    })

    it('should append transcript for terminal by ptyId', () => {
      const { setTerminalPtyId, appendTranscript } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-1')

      appendTranscript('pty-transcript-1', '\u001b[32mhello\u001b[0m')
      appendTranscript('pty-transcript-1', ' world')

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.transcript).toBe('\u001b[32mhello\u001b[0m world')
    })

    it('should trim transcript on a full CRLF boundary', () => {
      const { setTerminalPtyId, appendTranscript } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-crlf')

      const prefix = `header\r\n${'x'.repeat(499995)}`
      appendTranscript('pty-transcript-crlf', prefix)
      appendTranscript('pty-transcript-crlf', 'tail')

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.transcript?.startsWith('\n')).toBe(false)
    })

    it('should consume transcript once and clear it', () => {
      const { setTerminalPtyId, appendTranscript, consumeTranscript } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-1')
      appendTranscript('pty-transcript-1', 'chunk')

      expect(consumeTranscript('pty-transcript-1')).toBe('chunk')
      expect(consumeTranscript('pty-transcript-1')).toBe('')
    })

    it('should allow replay code to peek transcript before consuming it', () => {
      const { setTerminalPtyId, appendTranscript, peekTranscript, consumeTranscript } =
        useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-1')
      appendTranscript('pty-transcript-1', 'chunk')

      expect(peekTranscript('pty-transcript-1')).toBe('chunk')
      expect(consumeTranscript('pty-transcript-1')).toBe('chunk')
      expect(peekTranscript('pty-transcript-1')).toBe('')
    })
  })

  describe('updateTerminalScrollback', () => {
    it('should update pendingScrollback field when called with scrollback array', () => {
      const { updateTerminalScrollback } = useTerminalStore.getState()

      updateTerminalScrollback('t1', ['line 1', 'line 2', 'line 3'])

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(terminal?.pendingScrollback).toEqual(['line 1', 'line 2', 'line 3'])
    })

    it('should set pendingScrollback to undefined when called with undefined', () => {
      const { updateTerminalScrollback } = useTerminalStore.getState()

      // First set a value
      updateTerminalScrollback('t1', ['existing line 1', 'existing line 2'])
      expect(
        useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.pendingScrollback
      ).toHaveLength(2)

      // Then clear it
      updateTerminalScrollback('t1', undefined)
      expect(
        useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.pendingScrollback
      ).toBeUndefined()
    })

    it('should not affect other terminals', () => {
      const { updateTerminalScrollback } = useTerminalStore.getState()

      updateTerminalScrollback('t1', ['terminal 1 lines'])

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      const terminal2 = terminals.find((t) => t.id === 't2')

      expect(terminal1?.pendingScrollback).toEqual(['terminal 1 lines'])
      expect(terminal2?.pendingScrollback).toBeUndefined()
    })

    it('should handle non-existent terminal id gracefully', () => {
      const { updateTerminalScrollback } = useTerminalStore.getState()

      // Should not throw
      expect(() => updateTerminalScrollback('non-existent-id', ['lines'])).not.toThrow()

      const { terminals } = useTerminalStore.getState()
      expect(terminals).toHaveLength(3)
    })
  })

  describe('updateTerminalActivity', () => {
    it('should set hasActivity to true', () => {
      const { updateTerminalActivity } = useTerminalStore.getState()

      updateTerminalActivity('t1', true)

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(terminal?.hasActivity).toBe(true)
    })

    it('should set hasActivity to false', () => {
      const { updateTerminalActivity } = useTerminalStore.getState()

      // First set to true
      updateTerminalActivity('t1', true)
      expect(useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.hasActivity).toBe(
        true
      )

      // Then set to false
      updateTerminalActivity('t1', false)
      expect(useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.hasActivity).toBe(
        false
      )
    })

    it('should not affect other terminals', () => {
      const { updateTerminalActivity } = useTerminalStore.getState()

      updateTerminalActivity('t1', true)

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      const terminal2 = terminals.find((t) => t.id === 't2')

      expect(terminal1?.hasActivity).toBe(true)
      expect(terminal2?.hasActivity).toBeUndefined()
    })
  })

  describe('retention policies', () => {
    it('truncates transcripts to the configured max chars while preserving the newest content', () => {
      const { setTerminalPtyId, appendTranscript } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-retention')

      const prefix = 'stale-header\n'
      const retainedBody = 'x'.repeat(MAX_TRANSCRIPT_CHARS)
      appendTranscript('pty-transcript-retention', prefix + retainedBody)

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.transcript?.length).toBe(MAX_TRANSCRIPT_CHARS)
      expect(terminal?.transcript?.startsWith('x')).toBe(true)
      expect(terminal?.transcript?.endsWith('x')).toBe(true)
      expect(terminal?.transcript?.includes('stale-header')).toBe(false)
      // The cached-remount replay writes the transcript raw onto a live
      // instance and has no heuristic to fall back on, so a trim that ate a
      // DEC mode transition is the one silently wrong replay. Marking it is
      // what lets the telemetry tell that case apart from a clean restore.
      expect(terminal?.transcriptTrimmed).toBe(true)
    })

    it('leaves a transcript that fits under the cap unmarked', () => {
      const { setTerminalPtyId, appendTranscript } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-untrimmed')

      appendTranscript('pty-transcript-untrimmed', 'short body\n')

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.transcript).toBe('short body\n')
      expect(terminal?.transcriptTrimmed).toBeFalsy()
    })

    it('clears the trimmed mark when the transcript is consumed', () => {
      const { setTerminalPtyId, appendTranscript, consumeTranscript } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-transcript-consume')

      appendTranscript(
        'pty-transcript-consume',
        `stale-header\n${'x'.repeat(MAX_TRANSCRIPT_CHARS)}`
      )
      expect(
        useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.transcriptTrimmed
      ).toBe(true)

      consumeTranscript('pty-transcript-consume')

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.transcript).toBeUndefined()
      expect(terminal?.transcriptTrimmed).toBeUndefined()
    })

    it('truncates hidden buffers after the configured delay and keeps transcript aligned', () => {
      vi.useFakeTimers()
      try {
        vi.setSystemTime(new Date('2026-05-02T00:00:00.000Z'))

        const {
          updateTerminalScrollback,
          setTerminalHidden,
          setAppHidden,
          truncateHiddenTerminalBuffers
        } = useTerminalStore.getState()

        const largeScrollback = Array.from(
          { length: TRUNCATED_BUFFER_SIZE + 25 },
          (_, index) => `line-${index + 1}`
        )
        const transcriptWithAnsi = largeScrollback
          .map((line, index) => `\u001b[3${index % 7}m${line}\u001b[0m`)
          .join('\n')
        updateTerminalScrollback('t1', largeScrollback)
        useTerminalStore.setState((state) => ({
          terminals: state.terminals.map((terminal) =>
            terminal.id === 't1' ? { ...terminal, transcript: transcriptWithAnsi } : terminal
          )
        }))

        setTerminalHidden('t1', true)
        setAppHidden(true)
        vi.advanceTimersByTime(HIDDEN_BUFFER_TRUNCATION_DELAY + 1)
        truncateHiddenTerminalBuffers()

        const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
        expect(terminal?.pendingScrollback).toHaveLength(TRUNCATED_BUFFER_SIZE)
        expect(terminal?.pendingScrollback?.[0]).toBe(`line-26`)
        expect(terminal?.pendingScrollback?.at(-1)).toBe(`line-${TRUNCATED_BUFFER_SIZE + 25}`)
        expect(terminal?.transcript).toContain('\u001b[')
        expect(terminal?.transcript).not.toBe(terminal?.pendingScrollback?.join('\n'))
      } finally {
        vi.useRealTimers()
      }
    })

    it('truncates transcript-only hidden terminals after the configured delay', () => {
      vi.useFakeTimers()
      try {
        vi.setSystemTime(new Date('2026-05-02T00:00:00.000Z'))

        const { setTerminalPtyId, appendTranscript, setAppHidden, truncateHiddenTerminalBuffers } =
          useTerminalStore.getState()
        setTerminalPtyId('t1', 'pty-hidden-transcript-only')
        appendTranscript(
          'pty-hidden-transcript-only',
          Array.from(
            { length: TRUNCATED_BUFFER_SIZE + 10 },
            (_, index) => `line-${index + 1}`
          ).join('\n')
        )

        setAppHidden(true)
        vi.advanceTimersByTime(HIDDEN_BUFFER_TRUNCATION_DELAY + 1)
        truncateHiddenTerminalBuffers()

        const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
        expect(terminal?.transcript?.split(/\r\n|\r|\n/)).toHaveLength(TRUNCATED_BUFFER_SIZE)
      } finally {
        vi.useRealTimers()
      }
    })

    // A transcript is a raw PTY byte stream. The hidden-buffer sweep used to
    // split it on line breaks and re-join with '\n', which rewrote every CR and
    // — because the result then differed from the input — marked the record
    // trimmed even when nothing had been dropped. The cached-remount replay
    // discards a trimmed transcript outright, so that false mark is what left a
    // switched-back terminal showing the frame the user left.
    it('leaves a CRLF transcript within the line budget byte-identical and unmarked', () => {
      vi.useFakeTimers()
      try {
        vi.setSystemTime(new Date('2026-05-02T00:00:00.000Z'))

        const { setTerminalPtyId, appendTranscript, setAppHidden, truncateHiddenTerminalBuffers } =
          useTerminalStore.getState()
        setTerminalPtyId('t1', 'pty-hidden-crlf-within-budget')
        // Over TRUNCATED_BUFFER_SIZE *characters* (which is what the sweep's
        // guard measures) but far under it in lines — the exact shape that used
        // to be rewritten for nothing.
        const transcript = Array.from(
          { length: 200 },
          (_, index) => `[32mline-${index + 1}[0m ${'padding'.repeat(6)}`
        ).join('\r\n')
        expect(transcript.length).toBeGreaterThan(TRUNCATED_BUFFER_SIZE)
        appendTranscript('pty-hidden-crlf-within-budget', transcript)

        setAppHidden(true)
        vi.advanceTimersByTime(HIDDEN_BUFFER_TRUNCATION_DELAY + 1)
        truncateHiddenTerminalBuffers()

        const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
        expect(terminal?.transcript).toBe(transcript)
        expect(terminal?.transcriptTrimmed).toBeFalsy()
      } finally {
        vi.useRealTimers()
      }
    })

    it('keeps CRLF intact when it really does drop the oldest lines', () => {
      vi.useFakeTimers()
      try {
        vi.setSystemTime(new Date('2026-05-02T00:00:00.000Z'))

        const { setTerminalPtyId, appendTranscript, setAppHidden, truncateHiddenTerminalBuffers } =
          useTerminalStore.getState()
        setTerminalPtyId('t1', 'pty-hidden-crlf-over-budget')
        appendTranscript(
          'pty-hidden-crlf-over-budget',
          Array.from(
            { length: TRUNCATED_BUFFER_SIZE + 10 },
            (_, index) => `line-${index + 1}`
          ).join('\r\n')
        )

        setAppHidden(true)
        vi.advanceTimersByTime(HIDDEN_BUFFER_TRUNCATION_DELAY + 1)
        truncateHiddenTerminalBuffers()

        const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
        expect(terminal?.transcript?.split('\r\n')).toHaveLength(TRUNCATED_BUFFER_SIZE)
        expect(terminal?.transcript?.startsWith('line-11\r\n')).toBe(true)
        expect(terminal?.transcript?.endsWith(`line-${TRUNCATED_BUFFER_SIZE + 10}`)).toBe(true)
        expect(terminal?.transcriptTrimmed).toBe(true)
      } finally {
        vi.useRealTimers()
      }
    })

    it('does not reset hidden timers on repeated hidden notifications', () => {
      vi.useFakeTimers()
      try {
        vi.setSystemTime(new Date('2026-05-02T00:00:00.000Z'))

        const { setAppHidden } = useTerminalStore.getState()
        setAppHidden(true)
        const firstHiddenSince = useTerminalStore.getState().terminals[0]?.appHiddenSince

        vi.advanceTimersByTime(1000)
        setAppHidden(true)
        const secondHiddenSince = useTerminalStore.getState().terminals[0]?.appHiddenSince

        expect(secondHiddenSince).toBe(firstHiddenSince)
      } finally {
        vi.useRealTimers()
      }
    })
  })

  describe('updateTerminalLastActivityTimestamp', () => {
    it('should update the lastActivityTimestamp', () => {
      const { updateTerminalLastActivityTimestamp } = useTerminalStore.getState()
      const timestamp = Date.now()

      updateTerminalLastActivityTimestamp('t1', timestamp)

      const { terminals } = useTerminalStore.getState()
      const terminal = terminals.find((t) => t.id === 't1')
      expect(terminal?.lastActivityTimestamp).toBe(timestamp)
    })

    it('should not affect other terminals', () => {
      const { updateTerminalLastActivityTimestamp } = useTerminalStore.getState()
      const timestamp = Date.now()

      updateTerminalLastActivityTimestamp('t1', timestamp)

      const { terminals } = useTerminalStore.getState()
      const terminal1 = terminals.find((t) => t.id === 't1')
      const terminal2 = terminals.find((t) => t.id === 't2')

      expect(terminal1?.lastActivityTimestamp).toBe(timestamp)
      expect(terminal2?.lastActivityTimestamp).toBeUndefined()
    })

    it('should overwrite existing timestamp', () => {
      const { updateTerminalLastActivityTimestamp } = useTerminalStore.getState()
      const firstTimestamp = 1000000
      const secondTimestamp = 2000000

      updateTerminalLastActivityTimestamp('t1', firstTimestamp)
      expect(
        useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.lastActivityTimestamp
      ).toBe(firstTimestamp)

      updateTerminalLastActivityTimestamp('t1', secondTimestamp)
      expect(
        useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.lastActivityTimestamp
      ).toBe(secondTimestamp)
    })
  })

  // ========== Multi-Project Terminal Preservation Tests ==========
  // These tests verify AC1, AC3, AC6 from the tech spec:
  // - AC1: Terminals are NOT killed when switching projects
  // - AC3: Terminals with live PTY reconnect when returning to project
  // - AC6: Workspace tabs remain stable across project switches

  describe('multi-project terminal preservation', () => {
    it('should preserve terminals from project A when project B becomes active', () => {
      // Setup: Terminals exist for both projects
      const { setTerminalPtyId } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-project-1-a')
      setTerminalPtyId('t2', 'pty-project-1-b')
      setTerminalPtyId('t3', 'pty-project-2')

      // Simulate switching to project 2
      useProjectStore.setState({ activeProjectId: '2' })

      // Verify: All terminals still exist
      const { terminals } = useTerminalStore.getState()
      const project1Terminals = terminals.filter((t) => t.projectId === '1')
      const project2Terminals = terminals.filter((t) => t.projectId === '2')

      // AC1: Terminals from project 1 should NOT be removed
      expect(project1Terminals.length).toBe(2)
      expect(project2Terminals.length).toBe(1)

      // AC3: ptyId bindings should be preserved
      expect(project1Terminals[0].ptyId).toBe('pty-project-1-a')
      expect(project1Terminals[1].ptyId).toBe('pty-project-1-b')
    })

    it('should find terminals by ptyId across multiple projects', () => {
      // Setup: Terminals with ptyIds across projects
      const { setTerminalPtyId, findTerminalByPtyId } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-cross-project-1')
      setTerminalPtyId('t3', 'pty-cross-project-2')

      // Test: findTerminalByPtyId should work regardless of which project is active
      const terminal1 = findTerminalByPtyId('pty-cross-project-1')
      const terminal2 = findTerminalByPtyId('pty-cross-project-2')

      expect(terminal1?.projectId).toBe('1')
      expect(terminal2?.projectId).toBe('2')
    })

    it('should maintain ptyIdIndex across project switches', () => {
      // Setup: Assign ptyIds
      const { setTerminalPtyId, findTerminalByPtyId } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-index-test-1')
      setTerminalPtyId('t3', 'pty-index-test-2')

      // Switch to project 2
      useProjectStore.setState({ activeProjectId: '2' })

      // The ptyIdIndex should still work for lookups
      const terminal = findTerminalByPtyId('pty-index-test-1')
      expect(terminal).toBeDefined()
      expect(terminal?.projectId).toBe('1')
    })

    it('should allow re-selecting terminals when returning to a project', () => {
      // Setup: Terminals exist for project 1 with ptyIds
      const { setTerminalPtyId, selectTerminal } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-return-test')
      setTerminalPtyId('t2', 'pty-return-test-2')

      // Switch to project 2
      useProjectStore.setState({ activeProjectId: '2' })
      useTerminalStore.setState({ activeTerminalId: 't3' })

      // Switch back to project 1
      useProjectStore.setState({ activeProjectId: '1' })

      // Select a terminal from project 1
      selectTerminal('t2')

      const { activeTerminalId, terminals } = useTerminalStore.getState()
      expect(activeTerminalId).toBe('t2')

      // Terminal should still have its ptyId
      const terminal = terminals.find((t) => t.id === 't2')
      expect(terminal?.ptyId).toBe('pty-return-test-2')
    })

    it('should maintain separate terminal lists per project', () => {
      const { terminals } = useTerminalStore.getState()

      // Verify terminals are properly associated with their projects
      const project1Ids = terminals.filter((t) => t.projectId === '1').map((t) => t.id)
      const project2Ids = terminals.filter((t) => t.projectId === '2').map((t) => t.id)

      expect(project1Ids).toEqual(expect.arrayContaining(['t1', 't2']))
      expect(project2Ids).toEqual(['t3'])

      // No overlap between projects
      const overlap = project1Ids.filter((id) => project2Ids.includes(id))
      expect(overlap).toHaveLength(0)
    })
  })

  describe('setTerminalNeedsAttention', () => {
    it('sets the needsAttention flag on the target terminal', () => {
      useTerminalStore.getState().setTerminalNeedsAttention('t2', true)

      const t2 = useTerminalStore.getState().terminals.find((t) => t.id === 't2')
      expect(t2?.needsAttention).toBe(true)
    })

    it('clears the needsAttention flag', () => {
      useTerminalStore.getState().setTerminalNeedsAttention('t2', true)
      useTerminalStore.getState().setTerminalNeedsAttention('t2', false)

      const t2 = useTerminalStore.getState().terminals.find((t) => t.id === 't2')
      expect(t2?.needsAttention).toBe(false)
    })

    it('does not affect other terminals', () => {
      useTerminalStore.getState().setTerminalNeedsAttention('t2', true)

      const t1 = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      const t3 = useTerminalStore.getState().terminals.find((t) => t.id === 't3')
      expect(t1?.needsAttention).toBeFalsy()
      expect(t3?.needsAttention).toBeFalsy()
    })

    it('is a no-op (same state reference) when the value is unchanged', () => {
      // Already false by default — setting false again must not produce a new array.
      const before = useTerminalStore.getState().terminals
      useTerminalStore.getState().setTerminalNeedsAttention('t2', false)
      const after = useTerminalStore.getState().terminals
      expect(after).toBe(before)
    })

    it('ignores unknown terminal ids', () => {
      const before = useTerminalStore.getState().terminals
      useTerminalStore.getState().setTerminalNeedsAttention('does-not-exist', true)
      const after = useTerminalStore.getState().terminals
      expect(after).toBe(before)
    })
  })

  describe('cleanup-only recovery', () => {
    const failure = (
      terminalId: string,
      cleanupStage: 'kill' | 'wait' | 'flusher_join' | 'reader_join' = 'reader_join'
    ) => ({
      success: false as const,
      code: 'TERMINATE_FAILED',
      error: JSON.stringify({
        terminalId,
        primaryCode: 'TERMINATE_FAILED',
        cleanupStage
      })
    })

    it('deduplicates sanitized records by retained terminal id and rejects extra-key secret shapes', () => {
      const { recordTerminalCleanupFailure } = useTerminalStore.getState()

      expect(recordTerminalCleanupFailure(failure('pty-cleanup-1', 'kill'))).toEqual({
        terminalId: 'pty-cleanup-1',
        primaryCode: 'TERMINATE_FAILED',
        cleanupStage: 'kill'
      })
      expect(recordTerminalCleanupFailure(failure('pty-cleanup-1', 'reader_join'))).not.toBeNull()
      expect(
        recordTerminalCleanupFailure({
          success: false,
          code: 'TERMINATE_FAILED',
          error: JSON.stringify({
            terminalId: 'pty-cleanup-2',
            primaryCode: 'TERMINATE_FAILED',
            cleanupStage: 'kill',
            claim: 'must-never-enter-renderer-recovery-state'
          })
        })
      ).toBeNull()

      const recoveries = useTerminalStore.getState().cleanupRecoveries
      expect(Object.keys(recoveries)).toEqual(['pty-cleanup-1'])
      expect(recoveries['pty-cleanup-1']).toEqual({
        terminalId: 'pty-cleanup-1',
        primaryCode: 'TERMINATE_FAILED',
        cleanupStage: 'reader_join',
        retrying: false,
        retryFailed: false
      })
      expect(JSON.stringify(recoveries)).not.toContain('claim')
    })

    it('coalesces double-click retries, retains failure, and never attaches, resumes, or spawns', async () => {
      const { recordTerminalCleanupFailure, retryTerminalCleanup } = useTerminalStore.getState()
      recordTerminalCleanupFailure(failure('pty-cleanup-retry'))

      let resolveRetry!: (value: ReturnType<typeof failure>) => void
      vi.mocked(terminalApi.terminate).mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveRetry = resolve
          })
      )

      const first = retryTerminalCleanup('pty-cleanup-retry')
      const second = retryTerminalCleanup('pty-cleanup-retry')
      expect(first).toBe(second)
      expect(terminalApi.terminate).toHaveBeenCalledTimes(1)
      expect(terminalApi.terminate).toHaveBeenCalledWith('pty-cleanup-retry')
      expect(useTerminalStore.getState().cleanupRecoveries['pty-cleanup-retry']?.retrying).toBe(
        true
      )

      resolveRetry(failure('pty-cleanup-retry', 'flusher_join'))
      await expect(first).resolves.toBe(false)

      expect(useTerminalStore.getState().cleanupRecoveries['pty-cleanup-retry']).toMatchObject({
        terminalId: 'pty-cleanup-retry',
        cleanupStage: 'flusher_join',
        retrying: false,
        retryFailed: true
      })
      expect(terminalApi.spawn).not.toHaveBeenCalled()
      expect(terminalApi.resume).not.toHaveBeenCalled()
    })

    it('successful destructive cleanup retry reconciles terminal record', async () => {
      useTerminalStore.setState((state) => ({
        terminals: state.terminals.map((terminal) =>
          terminal.id === 't1'
            ? { ...terminal, ptyId: 'pty-cleanup-success', claim: 'memory-only-claim' }
            : terminal
        ),
        ptyIdIndex: new Map([['pty-cleanup-success', 't1']]),
        activeTerminalId: 't1'
      }))
      const { recordTerminalCleanupFailure, retryTerminalCleanup } = useTerminalStore.getState()
      recordTerminalCleanupFailure(failure('pty-cleanup-success'))

      await expect(retryTerminalCleanup('pty-cleanup-success')).resolves.toBe(true)

      const state = useTerminalStore.getState()
      expect(state.cleanupRecoveries['pty-cleanup-success']).toBeUndefined()
      expect(state.terminals.find((terminal) => terminal.id === 't1')).toBeUndefined()
      expect(
        state.terminals.find((terminal) => terminal.ptyId === 'pty-cleanup-success')
      ).toBeUndefined()
      expect(state.ptyIdIndex.has('pty-cleanup-success')).toBe(false)
      expect(state.activeTerminalId).not.toBe('t1')
      expect(terminalApi.terminate).toHaveBeenCalledWith('pty-cleanup-success')
      expect(terminalApi.spawn).not.toHaveBeenCalled()
      expect(terminalApi.resume).not.toHaveBeenCalled()
    })
  })

  // ========== CAP-3: reclaimable terminal lease (claim) lifecycle ==========
  // The claim credential is in-memory only: set on spawn/rotate, cleared on
  // kill/close/restart/clearTerminalPtyId, and NEVER written to persistence.

  describe('CAP-3 terminal claim lifecycle', () => {
    it('setTerminalClaim sets the claim on the terminal owning the ptyId', () => {
      const { setTerminalPtyId, setTerminalClaim } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-claim')
      setTerminalClaim('pty-cap3-claim', 'lease-credential-1')

      const { terminals } = useTerminalStore.getState()
      expect(terminals.find((t) => t.id === 't1')?.claim).toBe('lease-credential-1')
      expect(terminals.find((t) => t.id === 't2')?.claim).toBeUndefined()
      expect(terminals.find((t) => t.id === 't3')?.claim).toBeUndefined()
    })

    it('setTerminalClaim is a no-op for an unknown ptyId', () => {
      const { setTerminalPtyId, setTerminalClaim } = useTerminalStore.getState()
      setTerminalPtyId('t1', 'pty-cap3-known')

      const before = useTerminalStore.getState().terminals
      setTerminalClaim('pty-cap3-unknown', 'lease-orphan')
      const after = useTerminalStore.getState().terminals

      // Same state reference: nothing was rewritten for a ptyId nobody owns.
      expect(after).toBe(before)
      expect(after.find((t) => t.id === 't1')?.claim).toBeUndefined()
      expect(after.some((t) => t.claim === 'lease-orphan')).toBe(false)
    })

    it('clears the claim when setTerminalClaim is called with undefined', () => {
      const { setTerminalPtyId, setTerminalClaim } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-clear')
      setTerminalClaim('pty-cap3-clear', 'lease-to-clear')
      expect(useTerminalStore.getState().terminals.find((t) => t.id === 't1')?.claim).toBe(
        'lease-to-clear'
      )

      setTerminalClaim('pty-cap3-clear', undefined)

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.claim).toBeUndefined()
      // The ptyId binding survives claim clearing — only the lease is gone.
      expect(terminal?.ptyId).toBe('pty-cap3-clear')
    })

    it('rotation semantics: setting a new claim replaces the previous credential', () => {
      const { setTerminalPtyId, setTerminalClaim } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-rotate')
      setTerminalClaim('pty-cap3-rotate', 'lease-generation-1')
      setTerminalClaim('pty-cap3-rotate', 'lease-generation-2')

      const { terminals } = useTerminalStore.getState()
      expect(terminals.find((t) => t.id === 't1')?.claim).toBe('lease-generation-2')
      // The rotated-out credential must not linger on any terminal record.
      expect(terminals.some((t) => t.claim === 'lease-generation-1')).toBe(false)
    })

    it('clearTerminalPtyId drops the claim bound to that PTY', () => {
      const { setTerminalPtyId, setTerminalClaim, clearTerminalPtyId } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-drop')
      setTerminalClaim('pty-cap3-drop', 'lease-bound-to-pty')

      clearTerminalPtyId('pty-cap3-drop')

      const terminal = useTerminalStore.getState().terminals.find((t) => t.id === 't1')
      expect(terminal?.ptyId).toBeUndefined()
      expect(terminal?.claim).toBeUndefined()
    })

    it('closeTerminal removes the record carrying the claim', () => {
      const { setTerminalPtyId, setTerminalClaim, closeTerminal } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-close')
      setTerminalClaim('pty-cap3-close', 'lease-on-closed-terminal')

      closeTerminal('t1', '1')

      const { terminals } = useTerminalStore.getState()
      expect(terminals.find((t) => t.id === 't1')).toBeUndefined()
      expect(terminals.some((t) => t.claim === 'lease-on-closed-terminal')).toBe(false)
    })

    it('restartTerminal preserves the live PTY claim during a renderer-only reset', () => {
      const { setTerminalPtyId, setTerminalClaim, restartTerminal } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-restart')
      setTerminalClaim('pty-cap3-restart', 'lease-stale-after-restart')

      restartTerminal('t1')

      const { terminals } = useTerminalStore.getState()
      const restarted = terminals.find((t) => t.id === 't1')
      // This compatibility action resets only renderer state. Explicit
      // restartTerminalResource owns PTY termination, re-spawn, and claim rotation.
      expect(restarted?.claim).toBe('lease-stale-after-restart')
      expect(restarted?.ptyId).toBe('pty-cap3-restart')
      expect(terminals.some((t) => t.claim === 'lease-stale-after-restart')).toBe(true)
    })

    it('excludes the claim from the auto-save persisted payload (persistence exclusion)', () => {
      const claimCredential = 'cap3-lease-credential-0123456789abcdef'
      const { setTerminalPtyId, setTerminalClaim, appendTranscript } = useTerminalStore.getState()

      setTerminalPtyId('t1', 'pty-cap3-persist')
      setTerminalClaim('pty-cap3-persist', claimCredential)
      appendTranscript('pty-cap3-persist', 'persisted output line 1\npersisted output line 2\n')

      const { terminals, activeTerminalId } = useTerminalStore.getState()
      const layout = serializeTerminalsForProject(terminals, '1', activeTerminalId)

      const persisted = layout.terminals.find((t) => t.id === 't1')
      expect(persisted).toBeDefined()
      // Ordinary continuity data still persists...
      expect(persisted?.transcript).toBe('persisted output line 1\npersisted output line 2\n')
      expect(persisted?.scrollback).toEqual(['persisted output line 1', 'persisted output line 2'])
      // ...but the lease credential never does.
      expect(persisted).not.toHaveProperty('claim')
      expect(persisted).not.toHaveProperty('ptyId')

      const serialized = JSON.stringify(layout)
      expect(serialized).not.toContain(claimCredential)
      expect(serialized).not.toContain('"claim"')
    })
  })

  describe('adoptRemoteProjectTerminal', () => {
    it('adds a phone-created project PTY without stealing the active tab', () => {
      const adoptedId = useTerminalStore.getState().adoptRemoteProjectTerminal({
        terminalId: 'pty-phone',
        projectId: '1',
        cwd: '/tmp/demo',
        cols: 80,
        rows: 24,
        shell: 'zsh'
      })

      expect(adoptedId).toBe('pty-phone')
      expect(useTerminalStore.getState().activeTerminalId).toBe('t1')
      expect(useTerminalStore.getState().findTerminalByPtyId('pty-phone')?.projectId).toBe('1')
      expect(
        useTerminalStore.getState().adoptRemoteProjectTerminal({
          terminalId: 'pty-phone',
          projectId: '1',
          cwd: '/tmp/demo',
          cols: 80,
          rows: 24,
          shell: 'zsh'
        })
      ).toBe('pty-phone')
      expect(
        useTerminalStore.getState().terminals.filter((item) => item.ptyId === 'pty-phone')
      ).toHaveLength(1)
    })
  })
})
