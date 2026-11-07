/** Host access is one global grant shared by every MCP client, not a user sandbox. */
export const MCP_SSH_OPERATIONS = [
  'profile_list',
  'profile_get',
  'test_connection',
  'execute',
  'list_files',
  'read_file',
  'write_file',
  'upload',
  'download',
  'job_start',
  'job_status',
  'job_list',
  'job_cancel'
] as const
export type McpSshOperation = (typeof MCP_SSH_OPERATIONS)[number]

export interface McpSshGrant {
  operations: McpSshOperation[]
  remoteRoots: string[]
  localRoots: string[]
  posixJobs: boolean
}
export interface McpHostAccessPolicy {
  schemaVersion: 1
  revision: number
  enabled: boolean
  workspace: { enabled: boolean; projectIds: string[] | null; includePaths: boolean }
  ssh: { profiles: Record<string, McpSshGrant> }
  profileFingerprints: Record<string, string>
}
export interface McpHostProfile {
  profileId: string
  name: string
  host: string
  port: number
  username: string
  authMethod: string
  tags: string[]
  fingerprint: string
}
export interface McpHostAccessView {
  policy: McpHostAccessPolicy
  profiles: McpHostProfile[]
  projects: { id: string; name: string }[]
  hostMode: string
}
export interface McpHostCall {
  provider: 'ssh' | 'workspace'
  tool: string
  arguments: Record<string, unknown>
}
export interface McpHostJob {
  jobId: string
  profileId: string
  state: string
  finished?: boolean
  remoteTerminationConfirmed?: boolean
}
export function emptyMcpHostAccessPolicy(): McpHostAccessPolicy {
  return {
    schemaVersion: 1,
    revision: 0,
    enabled: false,
    workspace: { enabled: false, projectIds: [], includePaths: false },
    ssh: { profiles: {} },
    profileFingerprints: {}
  }
}
export function emptyMcpSshGrant(): McpSshGrant {
  return { operations: [], remoteRoots: [], localRoots: [], posixJobs: false }
}
