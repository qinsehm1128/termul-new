import { brandCanonical } from '@shared/brand'

const COMPATIBILITY_WORKSPACE_DIRS = ['.qin-code/', '.se-manage/']

/** Default rules shown when creating a new Git project. */
export function getProjectGitignoreDefaults(): string {
  const actualWorkspaceDir = `${brandCanonical().workspaceDir.replace(/^\/+|\/+$/g, '')}/`
  return `${[...COMPATIBILITY_WORKSPACE_DIRS, actualWorkspaceDir]
    .filter((rule, index, rules) => rules.indexOf(rule) === index)
    .join('\n')}\n`
}
