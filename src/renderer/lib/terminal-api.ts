/**
 * Transport-neutral terminal facade.
 *
 * View attachment/detachment is deliberately independent from PTY resource
 * termination. Renderer refs route through the active runtime; neither helper
 * destroys a terminal.
 */

import type {
  IpcResult,
  PrimaryTerminalDataHandle,
  TerminalApi,
  TerminalResumeGrant,
  TerminalResumeRequest,
  TerminalScopedDataCallback
} from '@shared/types/ipc.types'
import { isTauriContext } from './tauri-runtime'
import {
  addRendererRef as addTauriRendererRef,
  createTauriTerminalApi,
  removeRendererRef as removeTauriRendererRef,
  setTerminalProtected as setTauriTerminalProtected
} from './tauri-terminal-api'
import { createWebTerminalApi, webTerminalInternals } from './web-terminal-api'

export const terminalApi: TerminalApi = isTauriContext()
  ? createTauriTerminalApi()
  : createWebTerminalApi()

/** Transport-neutral cold-resume entry point; never spawns or terminates a PTY. */
export function resumeTerminal(
  request: TerminalResumeRequest
): Promise<IpcResult<TerminalResumeGrant>> {
  return terminalApi.resume(request)
}

/** Subscribe directly to one PTY, falling back to transport-wide filtering. */
export function subscribeTerminalData(
  terminalId: string,
  callback: TerminalScopedDataCallback
): () => void {
  if (terminalApi.onDataForTerminal) {
    return terminalApi.onDataForTerminal(terminalId, callback)
  }
  return terminalApi.onData((candidateId, data) => {
    if (candidateId === terminalId) callback(data)
  })
}

/**
 * Claim the single live-writer slot for a PTY, binding the id later.
 *
 * The spawn path only learns its PTY id after the IPC round trip, so the
 * handler has to be registered before the id exists. Transports that predate
 * this contract degrade to id-filtered observation, which keeps them working
 * but does not give them the single-writer guarantee.
 */
export function registerPrimaryTerminalData(
  callback: TerminalScopedDataCallback
): PrimaryTerminalDataHandle {
  if (terminalApi.registerPrimaryTerminalData) {
    return terminalApi.registerPrimaryTerminalData(callback)
  }
  let boundTerminalId: string | null = null
  const unsubscribe = terminalApi.onData((candidateId, data) => {
    if (boundTerminalId !== null && candidateId === boundTerminalId) callback(data)
  })
  return {
    bind(terminalId: string): void {
      boundTerminalId = terminalId
    },
    dispose(): void {
      boundTerminalId = null
      unsubscribe()
    }
  }
}

export function addRendererRef(terminalId: string, rendererId: string): Promise<IpcResult<void>> {
  return isTauriContext()
    ? addTauriRendererRef(terminalId, rendererId)
    : webTerminalInternals.addRendererRef(terminalId, rendererId)
}

export function removeRendererRef(
  terminalId: string,
  rendererId: string
): Promise<IpcResult<void>> {
  return isTauriContext()
    ? removeTauriRendererRef(terminalId, rendererId)
    : webTerminalInternals.removeRendererRef(terminalId, rendererId)
}

export function setTerminalProtected(
  terminalId: string,
  protectedState: boolean
): Promise<IpcResult<void>> {
  return isTauriContext()
    ? setTauriTerminalProtected(terminalId, protectedState)
    : webTerminalInternals.setProtected(terminalId, protectedState)
}
