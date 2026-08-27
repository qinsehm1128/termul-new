import type { EditorWorkspaceList } from '@shared/types/editor-workspace.types'
import { parseEditorWorkspaceList } from '@shared/types/editor-workspace.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { webServerEditorWorkspaces } from './web-server-api'

export const webEditorWorkspaceApi = {
  async list(): Promise<IpcResult<EditorWorkspaceList>> {
    const raw = await webServerEditorWorkspaces.list()
    return parseListResult(raw)
  },
  async parseFile(path: string): Promise<IpcResult<EditorWorkspaceList>> {
    const raw = await webServerEditorWorkspaces.parse(path)
    return parseListResult(raw)
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
