import type {
  EditorWorkspaceList,
  ParseCodeWorkspaceRequest
} from '@shared/types/editor-workspace.types'
import { parseEditorWorkspaceList } from '@shared/types/editor-workspace.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'

export function createTauriEditorWorkspaceApi() {
  return {
    async list(): Promise<IpcResult<EditorWorkspaceList>> {
      const raw = await invoke<IpcResult<unknown>>('list_editor_workspaces')
      return parseListResult(raw)
    },
    async parseFile(path: string): Promise<IpcResult<EditorWorkspaceList>> {
      const payload: ParseCodeWorkspaceRequest = { path }
      const raw = await invoke<IpcResult<unknown>>('parse_code_workspace_file', { payload })
      return parseListResult(raw)
    }
  }
}

function parseListResult(raw: IpcResult<unknown>): IpcResult<EditorWorkspaceList> {
  if (!raw.success) {
    return { success: false, error: raw.error, code: raw.code }
  }
  const parsed = parseEditorWorkspaceList(raw.data)
  if (!parsed) {
    return { success: false, error: 'Invalid editor workspace payload', code: 'INVALID_PAYLOAD' }
  }
  return { success: true, data: parsed }
}
