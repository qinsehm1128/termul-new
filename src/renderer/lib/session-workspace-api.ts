import { isConversationId } from '@shared/types/conversation.types'
import type { IpcResult } from '@shared/types/ipc.types'
import type { SessionWorkspaceApi } from '@shared/types/session-workspace.types'
import { isTauriContext } from './tauri-runtime'
import { tauriSessionWorkspaceApi } from './tauri-session-workspace-api'
import { webSessionWorkspaceApi } from './web-session-workspace-api'

function invalidConversationId(): IpcResult<never> {
  return {
    success: false,
    error: 'conversationId must be a canonical lowercase-hyphenated UUID',
    code: 'CONVERSATION_INVALID_ID'
  }
}

/** Build the validation-only workspace facade over one exact transport singleton. */
export function createSessionWorkspaceApi(transport: SessionWorkspaceApi): SessionWorkspaceApi {
  return {
    getWorkspace(conversationId) {
      return isConversationId(conversationId)
        ? transport.getWorkspace(conversationId)
        : Promise.resolve(invalidConversationId())
    },
    writeWorkspace(conversationId, basedRevision, workspace) {
      if (!isConversationId(conversationId) || workspace.conversationId !== conversationId) {
        return Promise.resolve(invalidConversationId())
      }
      return transport.writeWorkspace(conversationId, basedRevision, workspace)
    },
    resolveRecovery: (request) => transport.resolveRecovery(request)
  }
}

const workspaceTransport = isTauriContext() ? tauriSessionWorkspaceApi : webSessionWorkspaceApi

/** Exact specialized workspace singleton imported by production hooks. */
export const sessionWorkspaceApi: SessionWorkspaceApi =
  createSessionWorkspaceApi(workspaceTransport)

export {
  createTauriSessionWorkspaceApi,
  tauriSessionWorkspaceApi
} from './tauri-session-workspace-api'
export { webSessionWorkspaceApi } from './web-session-workspace-api'
