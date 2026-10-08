import { beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * A menu split must land next to the pane the menu was opened on, whatever
 * pane the store happens to call active.
 *
 * Runs against the real workspace and terminal stores: the defect lives in the
 * hand-off between them, which the store-mocked suite in terminal-spawn.test.ts
 * cannot see. Only the host transport is faked.
 */

const { mockSpawn } = vi.hoisted(() => ({ mockSpawn: vi.fn() }))

vi.mock('@/lib/api', () => ({ terminalApi: { spawn: mockSpawn } }))

import { spawnTerminalInSplit } from '@/lib/terminal-spawn'
import { useTerminalStore } from '@/stores/terminal-store'
import {
  findPaneContainingTab,
  findParentSplit,
  terminalTabId,
  useWorkspaceStore
} from '@/stores/workspace-store'
import type { LeafNode, SplitNode } from '@/types/workspace.types'

function terminalLeaf(id: string, terminalId: string): LeafNode {
  const tabId = terminalTabId(terminalId)
  return {
    type: 'leaf',
    id,
    tabs: [{ type: 'terminal', id: tabId, terminalId }],
    activeTabId: tabId
  }
}

describe('spawnTerminalInSplit — split target', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useTerminalStore.setState({
      terminals: [],
      activeTerminalId: '',
      ptyIdIndex: new Map(),
      cleanupRecoveries: {}
    })
    const root: SplitNode = {
      type: 'split',
      id: 'row',
      direction: 'horizontal',
      sizes: [34, 33, 33],
      children: [
        terminalLeaf('pane-A', 'a'),
        terminalLeaf('pane-B', 'b'),
        terminalLeaf('pane-C', 'c')
      ]
    }
    // The user is working in A, but the store still names C as active — a
    // click inside a terminal body does not reach the pane's focus handler.
    useWorkspaceStore.setState({ root, activePaneId: 'pane-C', fullscreenPaneId: null })
    mockSpawn.mockResolvedValue({
      success: true,
      data: { id: 'pty-new', shell: 'bash', cwd: '/work' }
    })
  })

  /**
   * The host catalog can adopt the PTY before the spawn reply arrives, and it
   * files the tab under the active pane. The split then has to take the tab
   * from where it actually is: assuming it sits in A made the move a silent
   * no-op, leaving the new terminal as a tab in C with C focused.
   */
  it('splits next to the clicked pane even when the new tab was first filed under the active pane', async () => {
    mockSpawn.mockImplementation(async () => {
      const adoptedId = useTerminalStore.getState().adoptRemoteProjectTerminal({
        terminalId: 'pty-new',
        projectId: 'proj-1',
        cwd: '/work',
        cols: 80,
        rows: 24,
        shell: 'bash'
      })
      if (adoptedId) useWorkspaceStore.getState().ensureTerminalTab(adoptedId, undefined, false)
      return { success: true, data: { id: 'pty-new', shell: 'bash', cwd: '/work' } }
    })

    const result = await spawnTerminalInSplit('pane-A', 'proj-1', '/work', 'right')

    expect(result.success).toBe(true)
    const { root, activePaneId } = useWorkspaceStore.getState()
    const holder = findPaneContainingTab(root, terminalTabId(result.terminalId as string))
    expect(holder?.id).toBeDefined()
    expect(holder?.id).not.toBe('pane-A')
    expect(holder?.id).not.toBe('pane-C')
    const row = findParentSplit(root, holder?.id as string)
    const order = row?.children.map((child) => child.id)
    expect(order?.indexOf(holder?.id as string)).toBe((order?.indexOf('pane-A') ?? -2) + 1)
    expect(activePaneId).toBe(holder?.id)
    expect(findPaneContainingTab(root, terminalTabId('c'))?.tabs).toHaveLength(1)
  })

  it('splits next to the clicked pane on the ordinary spawn path', async () => {
    const result = await spawnTerminalInSplit('pane-A', 'proj-1', '/work', 'left')

    expect(result.success).toBe(true)
    const { root, activePaneId } = useWorkspaceStore.getState()
    const holder = findPaneContainingTab(root, terminalTabId(result.terminalId as string))
    const order = findParentSplit(root, holder?.id as string)?.children.map((child) => child.id)
    expect(order?.indexOf(holder?.id as string)).toBe((order?.indexOf('pane-A') ?? -2) - 1)
    expect(activePaneId).toBe(holder?.id)
  })
})
