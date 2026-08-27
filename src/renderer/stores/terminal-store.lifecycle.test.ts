import { readFileSync } from 'node:fs'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { terminalApi } from '@/lib/terminal-api'
import { useAcpStore } from '@/stores/acp-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'

vi.mock('@/lib/terminal-api', () => ({
  terminalApi: {
    closeView: vi.fn(async () => ({ success: true, data: undefined })),
    terminate: vi.fn(async () => ({ success: true, data: undefined })),
    spawn: vi.fn(async () => ({
      success: true,
      data: {
        id: 'pty-restarted',
        shell: 'bash',
        cwd: '/workspace',
        pid: 2,
        cols: 80,
        rows: 24,
        claim: 'fresh-in-memory-claim'
      }
    }))
  }
}))

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function terminalTabExists(): boolean {
  return getAllLeafPanes(useWorkspaceStore.getState().root).some((leaf) =>
    leaf.tabs.some((tab) => tab.type === 'terminal' && tab.terminalId === 'record-1')
  )
}

describe('Conversation terminal view/resource lifecycle', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useTerminalStore.setState({
      terminals: [
        {
          id: 'record-1',
          conversationId,
          projectId: 'project-1',
          name: 'Conversation terminal',
          shell: 'bash',
          ptyId: 'pty-1',
          claim: 'in-memory-claim',
          viewState: 'visible',
          healthStatus: 'running'
        }
      ],
      activeTerminalId: 'record-1',
      ptyIdIndex: new Map([['pty-1', 'record-1']]),
      cleanupRecoveries: {}
    })
    useWorkspaceStore.getState().resetLayout()
    useWorkspaceStore.getState().addTerminalTab('record-1')
    useAcpStore.setState({
      messages: {
        sessionOne: [
          {
            id: 'message-1',
            role: 'user',
            blocks: [{ type: 'text', text: 'preserve this chat' }],
            streaming: false,
            timestamp: 1
          }
        ]
      }
    })
  })

  it('close-view retains the record, claim, ptyId, and chat while removing only the tab', async () => {
    const beforeMessages = useAcpStore.getState().messages

    expect(await useTerminalStore.getState().closeTerminalView('record-1')).toBe(true)
    useWorkspaceStore.getState().closeTerminalView('record-1')

    const terminal = useTerminalStore.getState().terminals[0]
    expect(terminal).toMatchObject({
      id: 'record-1',
      conversationId,
      ptyId: 'pty-1',
      claim: 'in-memory-claim',
      viewState: 'hidden'
    })
    expect(terminalTabExists()).toBe(false)
    expect(useAcpStore.getState().messages).toBe(beforeMessages)
    expect(terminalApi.closeView).toHaveBeenCalledWith('pty-1')
    expect(terminalApi.terminate).not.toHaveBeenCalled()
  })

  it('reopen restores the view/tab without spawning or changing the claim cursor', async () => {
    await useTerminalStore.getState().closeTerminalView('record-1')
    useWorkspaceStore.getState().closeTerminalView('record-1')

    useWorkspaceStore.getState().reopenTerminalView('record-1')

    expect(terminalTabExists()).toBe(true)
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      ptyId: 'pty-1',
      claim: 'in-memory-claim',
      viewState: 'visible'
    })
    expect(terminalApi.terminate).not.toHaveBeenCalled()
  })

  it('explicit terminate removes only terminal resource state and leaves chat intact', async () => {
    const beforeMessages = useAcpStore.getState().messages

    expect(await useTerminalStore.getState().terminateTerminalResource('record-1')).toBe(true)
    useWorkspaceStore.getState().closeTerminalView('record-1')

    expect(useTerminalStore.getState().terminals).toEqual([])
    expect(useTerminalStore.getState().ptyIdIndex.has('pty-1')).toBe(false)
    expect(terminalTabExists()).toBe(false)
    expect(useAcpStore.getState().messages).toBe(beforeMessages)
    expect(terminalApi.terminate).toHaveBeenCalledWith('pty-1')
  })

  it('navigation close/reopen preserves cleanup-only recovery without making it live or spawning', async () => {
    useTerminalStore.getState().recordTerminalCleanupFailure({
      success: false,
      code: 'TERMINATE_FAILED',
      error: JSON.stringify({
        terminalId: 'pty-1',
        primaryCode: 'TERMINATE_FAILED',
        cleanupStage: 'reader_join'
      })
    })

    await useTerminalStore.getState().closeTerminalView('record-1')
    useWorkspaceStore.getState().closeTerminalView('record-1')
    useWorkspaceStore.getState().reopenTerminalView('record-1')

    expect(useTerminalStore.getState().cleanupRecoveries['pty-1']).toMatchObject({
      terminalId: 'pty-1',
      cleanupStage: 'reader_join',
      retrying: false
    })
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      id: 'record-1',
      ptyId: 'pty-1',
      claim: 'in-memory-claim'
    })
    expect(terminalApi.terminate).not.toHaveBeenCalled()
    expect(terminalApi.spawn).not.toHaveBeenCalled()
  })

  it('restart explicitly terminates and respawns in the same Conversation without touching chat', async () => {
    const beforeMessages = useAcpStore.getState().messages

    expect(await useTerminalStore.getState().restartTerminalResource('record-1')).toBe(true)

    expect(terminalApi.terminate).toHaveBeenCalledWith('pty-1')
    expect(terminalApi.spawn).toHaveBeenCalledWith(
      expect.objectContaining({
        conversationId,
        projectId: 'project-1',
        shell: 'bash'
      })
    )
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      id: 'record-1',
      conversationId,
      ptyId: 'pty-restarted',
      claim: 'fresh-in-memory-claim',
      viewState: 'visible'
    })
    expect(useAcpStore.getState().messages).toBe(beforeMessages)
  })

  it('guards navigation/view/unmount/disconnect/project-switch/shared-live from termination', () => {
    const guarded = [
      'src/renderer/App.tsx',
      'src/renderer/TauriApp.tsx',
      'src/renderer/layouts/WorkspaceLayout.tsx',
      'src/renderer/components/mobile/MobileChatShell.tsx',
      'src/renderer/components/terminal/ConnectedTerminal.tsx',
      'src/renderer/hooks/use-projects-persistence.ts',
      'src/renderer/hooks/use-terminal-resource-lifecycle.ts',
      'src/renderer/stores/workspace-store.ts',
      'src/renderer/lib/web-terminal-api.ts',
      'src/renderer/lib/tauri-terminal-api.ts',
      'src/renderer/stores/terminal-store.ts',
      'src-tauri/src/remote/host.rs'
    ]
    const forbidden =
      /terminalApi\.(?:kill|terminate)\s*\(|\bforce_kill\s*\(|\bkill_all\s*\(|\b(?:async\s+)?(?:kill|terminate)\s*\(/
    const allowedRegions: Record<string, Array<{ start: RegExp; end: RegExp; reason: string }>> = {
      'src/renderer/lib/web-terminal-api.ts': [
        {
          start: /async terminate\(terminalId\)/,
          end: /onData:/,
          reason: 'explicit terminate handler plus deprecated kill transport adapter'
        }
      ],
      'src/renderer/lib/tauri-terminal-api.ts': [
        {
          start: /async terminate\(terminalId:/,
          end: /onData\(callback:/,
          reason: 'explicit terminate handler plus deprecated kill Tauri adapter'
        }
      ],
      'src/renderer/stores/terminal-store.ts': [
        {
          start: /terminateTerminalResource: async/,
          end: /renameTerminal:/,
          reason: 'explicit user-confirmed terminate handler'
        },
        {
          start: /restartTerminalResource: async/,
          end: /updateTerminalActivityBatch:/,
          reason: 'explicit user-confirmed restart handler'
        }
      ]
    }
    const findings: string[] = []

    for (const file of guarded) {
      let source = readFileSync(file, 'utf8')
      if (file.endsWith('remote/host.rs')) source = source.split('#[cfg(test)]')[0]
      const lines = source.split('\n')
      let inBlockComment = false
      const strippedLines = lines.map((raw) => {
        let line = raw
        if (inBlockComment) {
          const end = line.indexOf('*/')
          if (end === -1) return ''
          line = line.slice(end + 2)
          inBlockComment = false
        }
        while (line.includes('/*')) {
          const start = line.indexOf('/*')
          const end = line.indexOf('*/', start + 2)
          if (end === -1) {
            line = line.slice(0, start)
            inBlockComment = true
            break
          }
          line = line.slice(0, start) + line.slice(end + 2)
        }
        return line.split('//')[0]
      })
      const allowedLineNumbers = new Set<number>()
      for (const region of allowedRegions[file] ?? []) {
        const start = strippedLines.findIndex((line) => region.start.test(line))
        const end = strippedLines.findIndex((line, index) => index > start && region.end.test(line))
        expect(
          start,
          `${file}: missing allowlist start for ${region.reason}`
        ).toBeGreaterThanOrEqual(0)
        expect(end, `${file}: missing allowlist end for ${region.reason}`).toBeGreaterThan(start)
        for (let index = start; index < end; index++) allowedLineNumbers.add(index + 1)
      }
      strippedLines.forEach((line, index) => {
        const lineNumber = index + 1
        if (forbidden.test(line) && !allowedLineNumbers.has(lineNumber)) {
          findings.push(`${file}:${lineNumber}: ${line.trim()}`)
        }
      })
    }

    expect(findings, findings.join('\n')).toEqual([])
  })
})
