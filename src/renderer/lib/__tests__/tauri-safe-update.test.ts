import { beforeEach, describe, expect, it } from 'vitest'
import { useTerminalStore } from '@/stores/terminal-store'
import { hasActiveTerminalSessions } from '../tauri-safe-update'

describe('tauri-safe-update', () => {
  beforeEach(() => {
    useTerminalStore.setState({
      terminals: [],
      activeTerminalId: ''
    })
  })

  it('returns false when there are no terminals', () => {
    expect(hasActiveTerminalSessions()).toBe(false)
  })

  it('returns false when all terminals are hidden', () => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 't1',
          name: 'Hidden 1',
          projectId: 'p1',
          shell: 'bash',
          isHidden: true,
          healthStatus: 'running'
        },
        {
          id: 't2',
          name: 'Hidden 2',
          projectId: 'p1',
          shell: 'bash',
          isHidden: true,
          healthStatus: 'running'
        }
      ]
    })

    expect(hasActiveTerminalSessions()).toBe(false)
  })

  it('returns false when terminals are hibernated', () => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 't1',
          name: 'Hibernated',
          projectId: 'p1',
          shell: 'bash',
          isHidden: false,
          healthStatus: 'hibernated'
        }
      ]
    })

    expect(hasActiveTerminalSessions()).toBe(false)
  })

  // A terminal whose process already ended cannot lose work to a restart, so
  // it must not keep raising the "running terminals" update warning.
  it.each([
    ['exited'],
    ['crashed']
  ])('returns false when the only terminal has %s', (healthStatus) => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 't1',
          name: 'Ended',
          projectId: 'p1',
          shell: 'bash',
          isHidden: false,
          healthStatus
        }
      ]
    } as never)

    expect(hasActiveTerminalSessions()).toBe(false)
  })

  it('returns true when at least one visible non-hibernated terminal exists', () => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 't1',
          name: 'Visible',
          projectId: 'p1',
          shell: 'bash',
          isHidden: false,
          healthStatus: 'running'
        },
        {
          id: 't2',
          name: 'Hidden',
          projectId: 'p1',
          shell: 'bash',
          isHidden: true,
          healthStatus: 'running'
        }
      ]
    })

    expect(hasActiveTerminalSessions()).toBe(true)
  })
})
