import { describe, expect, it } from 'vitest'
import {
  type EchoProbeTerminal,
  prepareCommandForHistory,
  redactCommandSecrets,
  wasLineEchoed
} from './command-capture'

/** A terminal whose cursor row renders `rendered`. */
function terminalShowing(rendered: string): EchoProbeTerminal {
  return {
    buffer: {
      active: {
        baseY: 10,
        cursorY: 2,
        getLine: (index: number) =>
          index === 12 ? { translateToString: (): string => rendered } : undefined
      }
    }
  }
}

describe('wasLineEchoed', () => {
  it('accepts a line the cursor row shows', () => {
    expect(wasLineEchoed(terminalShowing('~/project $ git status'), 'git status')).toBe(true)
  })

  it('rejects input the cursor row does not show, which is how a password prompt looks', () => {
    expect(wasLineEchoed(terminalShowing('[sudo] password for qs:'), 'hunter2patch')).toBe(false)
  })

  it('matches on the tail so a soft-wrapped long line still counts as echoed', () => {
    const typed = `echo ${'a'.repeat(200)}TAILPART`
    expect(wasLineEchoed(terminalShowing('aaaaTAILPART'), typed)).toBe(true)
  })

  it('fails closed without a terminal', () => {
    expect(wasLineEchoed(null, 'git status')).toBe(false)
    expect(wasLineEchoed(undefined, 'git status')).toBe(false)
  })

  it('fails closed when the buffer is missing', () => {
    expect(wasLineEchoed({}, 'git status')).toBe(false)
    expect(wasLineEchoed({ buffer: {} }, 'git status')).toBe(false)
  })

  it('fails closed when reading the row throws, as a disposed terminal does', () => {
    const exploding: EchoProbeTerminal = {
      buffer: {
        active: {
          baseY: 0,
          cursorY: 0,
          getLine: () => {
            throw new Error('terminal disposed')
          }
        }
      }
    }
    expect(wasLineEchoed(exploding, 'git status')).toBe(false)
  })

  it('rejects an empty line without consulting the buffer', () => {
    expect(wasLineEchoed(terminalShowing('$ '), '')).toBe(false)
  })
})

describe('redactCommandSecrets', () => {
  it('masks --password=value', () => {
    expect(redactCommandSecrets('psql --password=hunter2 -h db')).toBe(
      'psql --password=<redacted> -h db'
    )
  })

  it('masks a space-separated --token value', () => {
    expect(redactCommandSecrets('gh auth login --token ghp_abc123')).toBe(
      'gh auth login --token <redacted>'
    )
  })

  it('masks hyphenated and underscored key options alike', () => {
    expect(redactCommandSecrets('tool --api-key abc --access_key def')).toBe(
      'tool --api-key <redacted> --access_key <redacted>'
    )
  })

  it('masks credential-bearing environment assignments', () => {
    expect(redactCommandSecrets('export AWS_SECRET_ACCESS_KEY=abc123 && npm run deploy')).toBe(
      'export AWS_SECRET_ACCESS_KEY=<redacted> && npm run deploy'
    )
    expect(redactCommandSecrets('GITHUB_TOKEN=ghp_zzz gh pr list')).toBe(
      'GITHUB_TOKEN=<redacted> gh pr list'
    )
  })

  it('masks an Authorization header value', () => {
    expect(redactCommandSecrets(`curl -H 'Authorization: Bearer eyJhbGci' https://api`)).toBe(
      `curl -H 'Authorization: <redacted>' https://api`.replace('<redacted>', 'Bearer <redacted>')
    )
  })

  it('masks the attached -p password of the mysql family', () => {
    expect(redactCommandSecrets('mysql -uroot -phunter2 mydb')).toBe(
      'mysql -uroot -p<redacted> mydb'
    )
  })

  it('leaves -p alone for commands where it is not a password', () => {
    expect(redactCommandSecrets('mkdir -p src/nested/dir')).toBe('mkdir -p src/nested/dir')
    expect(redactCommandSecrets('docker run -p8080:80 nginx')).toBe('docker run -p8080:80 nginx')
    expect(redactCommandSecrets('cp -pr src dst')).toBe('cp -pr src dst')
  })

  it('leaves PWD alone — it is a directory, not a password', () => {
    expect(redactCommandSecrets('echo $PWD && OLDPWD=/tmp cd -')).toBe(
      'echo $PWD && OLDPWD=/tmp cd -'
    )
  })

  it('leaves an ordinary command untouched', () => {
    expect(redactCommandSecrets('git commit -m "fix: guard restore"')).toBe(
      'git commit -m "fix: guard restore"'
    )
  })
})

describe('prepareCommandForHistory', () => {
  it('returns the trimmed, redacted command for echoed input', () => {
    const terminal = terminalShowing('$ mysql -uroot -phunter2 mydb')
    expect(prepareCommandForHistory(terminal, '  mysql -uroot -phunter2 mydb  ')).toBe(
      'mysql -uroot -p<redacted> mydb'
    )
  })

  it('returns null for input the terminal never echoed', () => {
    expect(prepareCommandForHistory(terminalShowing('Password:'), 'hunter2patch')).toBeNull()
  })

  it('returns null for blank input', () => {
    expect(prepareCommandForHistory(terminalShowing('$ '), '   ')).toBeNull()
  })

  it('returns null when no terminal is available, rather than recording unverified input', () => {
    expect(prepareCommandForHistory(null, 'git status')).toBeNull()
  })
})
