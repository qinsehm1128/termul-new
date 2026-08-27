import {
  type CliResumeDefaultsV1,
  type CliSessionAgentId,
  parseCliResumeDefaults
} from '@shared/types/cli-session.types'
import { PersistenceKeys } from '@shared/types/persistence.types'

import { persistenceApi } from './persistence-api'

export async function loadCliResumeDefaults(): Promise<CliResumeDefaultsV1> {
  const result = await persistenceApi.read<unknown>(PersistenceKeys.cliResumeDefaults)
  if (!result.success) {
    return parseCliResumeDefaults(null)
  }
  return parseCliResumeDefaults(result.data)
}

export async function saveCliResumeDefaults(
  extraArgsByAgentId: Partial<Record<CliSessionAgentId, string>>
): Promise<void> {
  const payload = parseCliResumeDefaults({ schemaVersion: 1, extraArgsByAgentId })
  const result = await persistenceApi.write(PersistenceKeys.cliResumeDefaults, payload)
  if (!result.success) {
    throw new Error(result.error || 'Failed to save CLI resume defaults')
  }
}
