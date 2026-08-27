import { normalizeCwdForScope } from '@/lib/acp-history-persistence'

export type CliSessionScopeMode = 'directory' | 'project' | 'all'

export function uniqueNormalizedPaths(paths: Array<string | null | undefined>): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const path of paths) {
    if (!path) continue
    const normalized = normalizeCwdForScope(path)
    if (!normalized || seen.has(normalized)) continue
    seen.add(normalized)
    out.push(normalized)
  }
  return out
}

export function buildCliSessionScopePaths(input: {
  mode: CliSessionScopeMode
  directoryPath?: string | null
  projectPath?: string | null
  worktreePaths?: string[]
}): string[] | undefined {
  if (input.mode === 'all') return undefined
  if (input.mode === 'directory') {
    return uniqueNormalizedPaths([input.directoryPath ?? input.projectPath])
  }
  return uniqueNormalizedPaths([input.projectPath, ...(input.worktreePaths ?? [])])
}
