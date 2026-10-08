/**
 * Folders the user has cloned projects into, most recent first, offered again
 * the next time a project is cloned.
 */
import { PersistenceKeys } from '@shared/types/persistence.types'
import { persistenceApi } from '@/lib/api'

export const CLONE_PARENT_HISTORY_LIMIT = 8

export async function loadCloneParentHistory(): Promise<string[]> {
  const result = await persistenceApi.read<unknown>(PersistenceKeys.cloneParentDirs)
  if (!result.success || !Array.isArray(result.data)) return []
  return result.data
    .filter((dir): dir is string => typeof dir === 'string' && dir.trim().length > 0)
    .slice(0, CLONE_PARENT_HISTORY_LIMIT)
}

export function withCloneParent(history: string[], dir: string): string[] {
  const normalized = dir.trim().replace(/[\\/]+$/, '')
  return [normalized, ...history.filter((entry) => entry !== normalized)].slice(
    0,
    CLONE_PARENT_HISTORY_LIMIT
  )
}

export async function rememberCloneParent(dir: string): Promise<string[]> {
  const next = withCloneParent(await loadCloneParentHistory(), dir)
  await persistenceApi.write(PersistenceKeys.cloneParentDirs, next)
  return next
}
