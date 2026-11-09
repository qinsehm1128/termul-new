/** Grantable remote-device scopes. Host management is never a device permission. */
export const REMOTE_DEVICE_SCOPES = [
  'view',
  'terminal.input',
  'terminal.create',
  'agent.run',
  'files.write',
  'git.write',
  'screen.view',
  'screen.control'
] as const

export type RemoteDeviceScope = (typeof REMOTE_DEVICE_SCOPES)[number]

/** All timestamps are UNIX seconds, not JavaScript milliseconds. */
export interface RemoteDevice {
  id: string
  name: string
  platform: string
  createdAt: number
  lastSeen: number
  scopes: RemoteDeviceScope[]
  revokedAt: number | null
  legacy: boolean
  projectId?: string | null
  connectionCount: number
}

export interface RemoteDeviceList {
  devices: RemoteDevice[]
  revision: number
}

export interface RemoteDeviceAuditEntry {
  at: number
  event: string
  deviceId: string
  capability?: string | null
}

export interface RemotePairingCode {
  code: string
  expiresAt: number
}
