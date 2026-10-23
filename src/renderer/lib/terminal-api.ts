import type {
  ScreenObservationStatus,
  TerminalScreenReadResult
} from '@shared/types/terminal-screen.types'
import { legacyScreenObservation } from '@shared/types/terminal-screen.types'
import { useScreenObservationStore } from '@/stores/screen-observation-store'
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
import { trackLocalTerminalSpawn } from './local-terminal-spawns'
import { isTauriContext } from './tauri-runtime'
import {
  addRendererRef as addTauriRendererRef,
  createTauriTerminalApi,
  removeRendererRef as removeTauriRendererRef,
  setTerminalProtected as setTauriTerminalProtected
} from './tauri-terminal-api'
import { createWebTerminalApi, webTerminalInternals } from './web-terminal-api'

const runtimeTerminalApi: TerminalApi = isTauriContext()
  ? createTauriTerminalApi()
  : createWebTerminalApi()
const runtimeSpawn = runtimeTerminalApi.spawn.bind(runtimeTerminalApi)

/**
 * Every spawn from this renderer is marked local, so the host terminal catalog
 * does not adopt it a second time as a terminal opened on another device.
 */
const runtimeAttach = runtimeTerminalApi.attach.bind(runtimeTerminalApi)
const runtimeList = runtimeTerminalApi.list?.bind(runtimeTerminalApi)
const runtimeTerminate = runtimeTerminalApi.terminate.bind(runtimeTerminalApi)
const runtimeWatch = runtimeTerminalApi.watch?.bind(runtimeTerminalApi)

export const terminalApi: TerminalApi = Object.assign(runtimeTerminalApi, {
  spawn: async (options?: Parameters<TerminalApi['spawn']>[0]) => {
    const result = await trackLocalTerminalSpawn(() => runtimeSpawn(options))
    if (result.success) useScreenObservationStore.getState().recordHost(result.data.id, result.data)
    return result
  },
  attach: async (...args: Parameters<TerminalApi['attach']>) => {
    const result = await runtimeAttach(...args)
    if (result.success)
      useScreenObservationStore
        .getState()
        .recordHost(result.data.id, { ...result.data, claim: args[1] })
    return result
  },
  list: runtimeList
    ? async () => {
        const result = await runtimeList()
        if (result.success)
          for (const terminal of result.data) {
            useScreenObservationStore.getState().recordHost(terminal.id, terminal)
          }
        return result
      }
    : undefined,
  watch: runtimeWatch
    ? async (...args: Parameters<NonNullable<TerminalApi['watch']>>) => {
        const result = await runtimeWatch(...args)
        if (result.success)
          useScreenObservationStore.getState().recordHost(result.data.id, result.data)
        return result
      }
    : undefined,
  terminate: async (terminalId: string) => {
    const result = await runtimeTerminate(terminalId)
    if (result.success) useScreenObservationStore.getState().clear(terminalId)
    return result
  }
})

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

/** Explicit, claim/scope-protected host status read; no background polling. */
export function readTerminalScreenStatus(
  terminalId: string,
  claim: string
): Promise<IpcResult<ScreenObservationStatus>> {
  return (
    terminalApi.screenStatus?.(terminalId, claim) ??
    Promise.resolve({ success: true, data: legacyScreenObservation() })
  )
}

export function readTerminalScreen(
  terminalId: string,
  claim: string,
  maxLines?: number
): Promise<IpcResult<TerminalScreenReadResult>> {
  return (
    terminalApi.screenRead?.(terminalId, claim, maxLines) ??
    Promise.resolve({
      success: true,
      data: {
        status: legacyScreenObservation(),
        text: null,
        appliedSeq: null,
        truncated: false
      }
    })
  )
}
