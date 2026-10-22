import type { QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { normalizeCwdForScope } from '@/lib/acp-history-persistence'
import type { Project } from '@/types/project'

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

/**
 * Which directory "this directory" means when listing past agent sessions.
 *
 * Precedence is the subject the user is looking at, narrowest first:
 *
 * 1. an open Conversation's own workspace — it is often outside any registered
 *    project, and it is where that Conversation's agents actually ran;
 * 2. the focused terminal's cwd;
 * 3. the active project's default cwd.
 *
 * Conversation first is the whole point: without it, opening the panel inside a
 * Conversation lists the *project's* history instead of the folder on screen.
 */
export function resolveCliSessionDirectory(input: {
  conversationWorkspaceCwd?: string | null
  terminalCwd?: string | null
  projectDefaultCwd?: string | null
}): string | null {
  return input.conversationWorkspaceCwd || input.terminalCwd || input.projectDefaultCwd || null
}

/**
 * What "this directory" and "this project" mean on a quick terminal's page.
 *
 * The quick terminal is the subject there, not the project that was active
 * before: its own folder is the directory, and the project is the one it was
 * opened in, or, for a private folder, that folder itself.
 */
export function quickTerminalCliSessionScope(
  record: QuickTerminalRecord,
  projects: Pick<Project, 'id' | 'path' | 'worktrees'>[]
): { directoryPath: string; projectPath: string; worktreePaths: string[] } {
  const { target } = record
  if (target.kind === 'workspace') {
    return { directoryPath: record.cwd, projectPath: record.cwd, worktreePaths: [] }
  }
  const project = projects.find((candidate) => candidate.id === target.projectId)
  return {
    directoryPath: record.cwd,
    projectPath:
      project?.path ?? (target.kind === 'project_root' ? target.projectRoot : target.worktreePath),
    worktreePaths: project?.worktrees?.map((worktree) => worktree.path) ?? []
  }
}
