import type { ConversationApi } from '@shared/types/conversation-api.types'
import type { ConversationLifecycleApi } from '@shared/types/conversation-lifecycle.types'
import type { SessionWorkspaceApi } from '@shared/types/session-workspace.types'
import { conversationLifecycleApi } from './conversation-lifecycle-api'
import { sessionWorkspaceApi } from './session-workspace-api'
import { tauriConversationApi } from './tauri-conversation-api'
import { isTauriContext } from './tauri-runtime'
import { webConversationApi } from './web-conversation-api'

export type ConversationFacadeApi = ConversationApi &
  SessionWorkspaceApi &
  Omit<ConversationLifecycleApi, 'subscribe'>

const coreConversationApi = isTauriContext() ? tauriConversationApi : webConversationApi

/**
 * Compatibility facade with no transport logic. Every specialized domain delegates to the exact
 * production singleton that its dedicated hooks and stores import directly.
 */
export function createConversationFacadeApi(
  coreApi: ConversationApi = coreConversationApi,
  workspaceApi: SessionWorkspaceApi = sessionWorkspaceApi,
  lifecycleApi: ConversationLifecycleApi = conversationLifecycleApi
): ConversationFacadeApi {
  return {
    getHostStatus: () => coreApi.getHostStatus(),
    listConversations: () => coreApi.listConversations(),
    getConversation: (conversationId) => coreApi.getConversation(conversationId),
    getCurrentBinding: (conversationId) => coreApi.getCurrentBinding(conversationId),
    openConversation: (conversationId) => coreApi.openConversation(conversationId),
    renameConversation: (conversationId, title) =>
      coreApi.renameConversation(conversationId, title),
    resolveLegacyConversationId: (key) => coreApi.resolveLegacyConversationId(key),
    attachProject: (conversationId, expectedRevision, attachment) =>
      coreApi.attachProject(conversationId, expectedRevision, attachment),
    detachProject: (conversationId, expectedRevision) =>
      coreApi.detachProject(conversationId, expectedRevision),
    updateExecutionTarget: (conversationId, expectedRevision, executionTarget) =>
      coreApi.updateExecutionTarget(conversationId, expectedRevision, executionTarget),
    subscribeHostStatus: (listener) => coreApi.subscribeHostStatus(listener),
    getWorkspace: (conversationId) => workspaceApi.getWorkspace(conversationId),
    writeWorkspace: (conversationId, basedRevision, workspace) =>
      workspaceApi.writeWorkspace(conversationId, basedRevision, workspace),
    resolveRecovery: (request) => workspaceApi.resolveRecovery(request),
    detachBinding: (conversationId, expectedRevision) =>
      lifecycleApi.detachBinding(conversationId, expectedRevision),
    rebindDetachedBinding: (conversationId, expectedRevision) =>
      lifecycleApi.rebindDetachedBinding(conversationId, expectedRevision),
    suspendBinding: (conversationId, expectedRevision) =>
      lifecycleApi.suspendBinding(conversationId, expectedRevision),
    replaceBinding: (conversationId, request, expectedRevision, targetRuntimeAgentId) =>
      lifecycleApi.replaceBinding(conversationId, request, expectedRevision, targetRuntimeAgentId),
    deleteConversation: (conversationId, expectedRevision) =>
      lifecycleApi.deleteConversation(conversationId, expectedRevision)
  }
}

export const conversationApi: ConversationFacadeApi = createConversationFacadeApi()

export {
  createTauriConversationApi,
  tauriConversationApi
} from './tauri-conversation-api'
export { createWebConversationApi, webConversationApi } from './web-conversation-api'
