import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockDispose } = vi.hoisted(() => ({ mockDispose: vi.fn() }))

vi.mock('@/lib/acp-api', () => ({
  acpApi: { disposeEphemeralSession: mockDispose }
}))

import {
  _resetPreviousLoadDisposalForTests,
  disposeBackendEphemeralSessionsFromPreviousLoad,
  forgetBackendEphemeralSession,
  rememberBackendEphemeralSession,
  rememberedBackendEphemeralSessionIds
} from './backend-ephemeral-sessions'

describe('backend-ephemeral sessions', () => {
  beforeEach(() => {
    localStorage.clear()
    mockDispose.mockReset()
    mockDispose.mockResolvedValue(undefined)
    _resetPreviousLoadDisposalForTests()
  })

  it('remembers a session until it is forgotten', () => {
    rememberBackendEphemeralSession('agent-1', 'sess-1')
    rememberBackendEphemeralSession('agent-1', 'sess-2')
    forgetBackendEphemeralSession('sess-1')

    expect(rememberedBackendEphemeralSessionIds()).toEqual(['sess-2'])
  })

  it('disposes what a previous load left, once per load', async () => {
    rememberBackendEphemeralSession('agent-1', 'sess-old')

    await disposeBackendEphemeralSessionsFromPreviousLoad()
    // This load's own warm session must survive a second call.
    rememberBackendEphemeralSession('agent-1', 'sess-new')
    await disposeBackendEphemeralSessionsFromPreviousLoad()

    expect(mockDispose).toHaveBeenCalledTimes(1)
    expect(mockDispose).toHaveBeenCalledWith('agent-1', 'sess-old')
    expect(rememberedBackendEphemeralSessionIds()).toEqual(['sess-new'])
  })

  it('keeps going when a session was already ended', async () => {
    rememberBackendEphemeralSession('agent-1', 'sess-gone')
    rememberBackendEphemeralSession('agent-2', 'sess-live')
    mockDispose.mockRejectedValueOnce(new Error('session is not ephemeral'))

    await disposeBackendEphemeralSessionsFromPreviousLoad()

    expect(mockDispose).toHaveBeenCalledWith('agent-2', 'sess-live')
    expect(rememberedBackendEphemeralSessionIds()).toEqual([])
  })
})
