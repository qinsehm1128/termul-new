/**
 * Guards for the terminal command capture that feeds the command history.
 *
 * Capture reads the user's *keystrokes*, not what the shell actually ran, so it
 * needs two guards before anything reaches disk:
 *
 * - `wasLineEchoed` drops input the terminal never showed. A password prompt
 *   turns echo off, so the characters typed at one would otherwise be captured
 *   verbatim and written to a plaintext file.
 * - `redactCommandSecrets` masks credentials that were typed *into* a visible
 *   command line (`mysql -phunter2`, `export TOKEN=…`), which echo does not
 *   protect against.
 *
 * Both are required. Neither is sufficient alone.
 */

/** Replacement for a masked credential; deliberately not the original length. */
const REDACTED = '<redacted>'

/**
 * How many trailing characters of the typed line must appear on the cursor row
 * for the line to count as echoed.
 *
 * Trailing rather than leading: a long line soft-wraps, so only its tail is on
 * the cursor row. Short enough that a prompt redraw does not shift it out of
 * view, long enough that a stray one-character coincidence is not a match.
 */
const ECHO_PROBE_CHARS = 8

/** Minimal shape of the xterm buffer this module needs; keeps tests cheap. */
export interface EchoProbeTerminal {
  buffer?: {
    active?: {
      baseY: number
      cursorY: number
      getLine: (index: number) => { translateToString: (trimRight?: boolean) => string } | undefined
    }
  }
}

/**
 * Whether `line` is visible on the terminal's cursor row.
 *
 * Fails closed: no terminal, no buffer, or an unreadable row all return false.
 * Losing a history entry is recoverable; writing a password to disk is not.
 */
export function wasLineEchoed(
  terminal: EchoProbeTerminal | null | undefined,
  line: string
): boolean {
  if (!line) return false

  const buffer = terminal?.buffer?.active
  if (!buffer) return false

  let rendered: string
  try {
    const row = buffer.getLine(buffer.baseY + buffer.cursorY)
    if (!row) return false
    rendered = row.translateToString(true)
  } catch {
    // A half-initialised terminal (no canvas backend, disposed instance) throws
    // here rather than returning empty. Same fail-closed answer.
    return false
  }

  const probe = line.slice(-ECHO_PROBE_CHARS)
  return probe.length > 0 && rendered.includes(probe)
}

/**
 * Option names whose value is a credential.
 *
 * Matched case-insensitively against both `--name=value` and `--name value`.
 */
const SECRET_OPTION_NAMES =
  'password|passwd|token|secret|api[-_]?key|access[-_]?key|private[-_]?key|credential|auth[-_]?token'

/**
 * Environment-assignment keys whose value is a credential.
 *
 * `PWD` is intentionally absent — it is a directory, not a password.
 */
const SECRET_ASSIGNMENT_KEYS =
  '[A-Z0-9_]*(?:PASSWORD|PASSWD|SECRET|TOKEN|API_?KEY|ACCESS_?KEY|PRIVATE_?KEY|CREDENTIAL|AUTH)[A-Z0-9_]*'

/**
 * Commands whose `-p` takes an attached password.
 *
 * Scoped to this family on purpose: `-p` means "parents" to mkdir, "publish"
 * to docker, and "preserve" to cp. Masking those would corrupt useful history
 * to protect a secret that was never there.
 */
const ATTACHED_PASSWORD_COMMANDS = /^(mysql|mysqldump|mysqladmin|mariadb|mariadb-dump)\b/

const REDACTIONS: Array<[RegExp, string]> = [
  // --password=value / --token=value
  [new RegExp(`(--(?:${SECRET_OPTION_NAMES})=)\\S+`, 'gi'), `$1${REDACTED}`],
  // --password value / --token value
  [new RegExp(`(--(?:${SECRET_OPTION_NAMES})\\s+)\\S+`, 'gi'), `$1${REDACTED}`],
  // TOKEN=value / AWS_SECRET_ACCESS_KEY=value
  [new RegExp(`\\b(${SECRET_ASSIGNMENT_KEYS}=)\\S+`, 'g'), `$1${REDACTED}`],
  // Authorization: Bearer xxx  (inside a quoted header argument)
  [
    /((?:authorization|proxy-authorization)\s*:\s*(?:bearer|basic|token)\s+)[^'"\s]+/gi,
    `$1${REDACTED}`
  ]
]

/**
 * Mask credentials written on a command line.
 *
 * Returns the command with values replaced, preserving the command's shape —
 * the shape is what makes history useful for recall.
 */
export function redactCommandSecrets(command: string): string {
  let result = command

  for (const [pattern, replacement] of REDACTIONS) {
    result = result.replace(pattern, replacement)
  }

  if (ATTACHED_PASSWORD_COMMANDS.test(result.trimStart())) {
    // Only the attached form carries a secret; a bare `-p` prompts instead, and
    // that prompt is already covered by the echo guard.
    result = result.replace(/(\s-p)(\S+)/g, `$1${REDACTED}`)
  }

  return result
}

/**
 * Full capture gate: returns the entry to record, or null to record nothing.
 */
export function prepareCommandForHistory(
  terminal: EchoProbeTerminal | null | undefined,
  rawLine: string
): string | null {
  const trimmed = rawLine.trim()
  if (!trimmed) return null
  // Probe with the trimmed line: the rendered row is right-trimmed, so trailing
  // spaces the user typed would never appear on screen and would read as
  // "not echoed".
  if (!wasLineEchoed(terminal, trimmed)) return null
  return redactCommandSecrets(trimmed)
}
