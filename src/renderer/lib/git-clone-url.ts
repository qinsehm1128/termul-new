/**
 * Turn what a user pastes into a clone address and a folder name.
 *
 * Accepts the forms people actually copy: a GitHub page URL (including one
 * pointing into the tree, like `/tree/main/src`), an `https://…/.git` clone
 * URL, an SSH `git@host:owner/repo.git`, `github.com/owner/repo`, or the
 * `owner/repo` shorthand. The folder name is the repository name.
 */
export interface ParsedRepoUrl {
  cloneUrl: string
  repoName: string
}

const SEGMENT = /^[A-Za-z0-9._-]+$/

function repoNameFrom(last: string | undefined): string | null {
  const name = (last ?? '').replace(/\.git$/, '')
  if (!SEGMENT.test(name) || name === '.' || name === '..' || name.startsWith('-')) return null
  return name
}

export function parseGitRepoUrl(input: string): ParsedRepoUrl | null {
  const raw = input.trim().replace(/\/+$/, '')
  if (!raw || /\s/.test(raw) || raw.startsWith('-')) return null

  // `owner/repo` shorthand means GitHub.
  const shorthand = raw.split('/')
  if (shorthand.length === 2 && shorthand.every((part) => SEGMENT.test(part))) {
    const repoName = repoNameFrom(shorthand[1])
    return repoName
      ? { cloneUrl: `https://github.com/${shorthand[0]}/${repoName}.git`, repoName }
      : null
  }

  // SCP-like SSH: git@host:owner/repo.git
  const scp = /^git@([^:/]+):(.+)$/.exec(raw)
  if (scp) {
    const repoName = repoNameFrom(scp[2].split('/').pop())
    return repoName ? { cloneUrl: raw, repoName } : null
  }

  const withScheme = /^[a-z]+:\/\//i.test(raw) ? raw : `https://${raw}`
  let url: URL
  try {
    url = new URL(withScheme)
  } catch {
    return null
  }
  if (!['https:', 'http:', 'ssh:', 'git:'].includes(url.protocol)) return null
  const segments = url.pathname.split('/').filter(Boolean)
  if (segments.length < 2) return null

  // A GitHub page URL may point deeper (`/tree/main/…`, `/pull/1`); the
  // repository is always the first two segments.
  if (url.hostname === 'github.com') {
    const repoName = repoNameFrom(segments[1])
    return repoName
      ? { cloneUrl: `https://github.com/${segments[0]}/${repoName}.git`, repoName }
      : null
  }
  const repoName = repoNameFrom(segments[segments.length - 1])
  return repoName ? { cloneUrl: withScheme, repoName } : null
}

/** Where the clone will land, for showing before it happens. */
export function cloneTargetPath(parentDir: string, repoName: string): string {
  const parent = parentDir.trim().replace(/[\\/]+$/, '')
  const separator = parent.includes('\\') && !parent.includes('/') ? '\\' : '/'
  return `${parent}${separator}${repoName}`
}
