import type { EditorWorkspaceList } from '@shared/types/editor-workspace.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { createTauriEditorWorkspaceApi } from './tauri-editor-workspace-api'
import { isTauriContext } from './tauri-runtime'
import { webEditorWorkspaceApi } from './web-editor-workspace-api'

export interface EditorWorkspaceApi {
  list(): Promise<IpcResult<EditorWorkspaceList>>
  parseFile(path: string): Promise<IpcResult<EditorWorkspaceList>>
}

const tauriEditorWorkspaceApi = createTauriEditorWorkspaceApi()

export const editorWorkspaceApi: EditorWorkspaceApi = {
  list() {
    if (!isTauriContext()) return webEditorWorkspaceApi.list()
    return tauriEditorWorkspaceApi.list()
  },
  parseFile(path: string) {
    if (!isTauriContext()) return webEditorWorkspaceApi.parseFile(path)
    return tauriEditorWorkspaceApi.parseFile(path)
  }
}

export { createTauriEditorWorkspaceApi } from './tauri-editor-workspace-api'
export { webEditorWorkspaceApi } from './web-editor-workspace-api'
