import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { performance } from 'node:perf_hooks'
import { describe, expect, it, vi } from 'vitest'
import {
  checkConversationFirstGuardrails,
  type GuardFinding,
  type GuardSources,
  loadRepositorySources,
  main,
  stripComments,
  stripRustTestCode
} from './check-conversation-first-guardrails'

const hooks = [
  'useTerminalAutoSave',
  'useSessionWorkspaceBootstrap',
  'useConversationHostBootstrap',
  'useConversationLifecycle',
  'useTerminalResourceLifecycle',
  'useTerminalRestore',
  'useCrashRecovery',
  'useTerminalDetachedOutput',
  'useCwd',
  'useGitBranch',
  'useGitStatus',
  'useExitCode',
  'useContextBarSettings',
  'useAppSettingsLoader',
  'useAppliedLanguageSync',
  'useAppliedColorThemeSync',
  'useAppliedUiZoomSync',
  'useKeyboardShortcutsLoader',
  'useProjectsLoader',
  'useProjectsAutoSave',
  'useMenuUpdaterListener',
  'useUpdateCheck',
  'useUpdateToast',
  'useVisibilityState',
  'useTerminalExitNotification',
  'useRemoteProjects',
  'useAcpListeners',
  'useAcpAgents',
  'useAcpHistory',
  'useAcpSessionResume',
  'useAcpMcp',
  'usePreventFileDropNavigation',
  'usePreventNativeContextMenu'
]

const syncRun = `python3 scripts/sync-stamped-root-lock.py --manifest src-tauri/Cargo.toml --lockfile src-tauri/Cargo.lock --package termul-manager
cargo metadata --locked --manifest-path src-tauri/Cargo.toml --format-version 1 --no-deps`

function root(): string {
  return `
import { PortableAppEffects as Effects } from '@/app/PortableAppEffects'
import { createPortableRouter as buildRouter } from '@/app/portable-router'
import { ConversationHostStatus as Status } from '@/components/conversation/ConversationHostStatus'
import { ConversationRecoveryPanel as Recovery } from '@/components/conversation/ConversationRecoveryPanel'
const makeRouter = buildRouter
const router = makeRouter()
export default function Root() {
  return <><Effects /><Status /><Recovery /></>
}
`
}

function portableEffects(): string {
  return `
${hooks.map((hook) => `import { ${hook} as ${hook}Alias } from '@/hooks/${hook}'`).join('\n')}
import { initNotificationPermissions as initializeNotifications } from '@/lib/tauri-notification-api'
export function PortableAppEffects() {
${hooks.map((hook) => `  ${hook}Alias()`).join('\n')}
  initializeNotifications()
  return null
}
`
}

function portableRouter(): string {
  return `
import { createHashRouter } from 'react-router-dom'
export const portableRouteObjects = [{ children: [
  { path: 'c/:conversationId' },
  { path: 'legacy/session/:legacyValue' },
  { path: 'legacy/storage/:legacyValue' },
  { path: 'legacy/history/:legacyValue' },
  { path: 'snapshots' },
  { path: 'settings' },
  { path: 'preferences' }
]}]
export function createPortableRouter() { return createHashRouter(portableRouteObjects) }
`
}

function prWorkflow(extra = ''): string {
  return `
jobs:
  validate:
    steps:
      - name: Check conversation-first guardrails
        run: bun run check:conversation-first
  conversation-native-durability:
    strategy:
      matrix:
        include:
          - platform: linux
          - platform: macos
          - platform: windows
    steps:
      - run: cargo test --locked conversation::native_durability_tests
  rust-checks:
    steps:
      - run: cargo test --locked --test conversation_first_guardrails
  standalone-server-build:
    steps:
      - run: cargo build --locked --bin termul-server --features standalone-server
      - run: cargo clippy --locked --bin termul-server --features standalone-server -- -D warnings
  rust-windows-check:
    steps:
      - run: cargo test --locked web::auth::tests::windows_token_descriptor_rejects_foreign_owner_null_dacl_and_broad_allow_ace -- --exact
${extra}`
}

function packagingWorkflow(stamp: string): string {
  return `
jobs:
  build:
    steps:
      - name: ${stamp}
        run: stamp-version
      - name: Synchronize stamped root lock entry
        run: |-
          ${syncRun.replace('\n', '\n          ')}
      - name: Build platform artifacts locally
        uses: tauri-apps/tauri-action@pinned
        with:
          args: \${{ matrix.args }} --config src-tauri/tauri.conf.prod.json -- --locked
  standalone-server:
    steps:
      - name: ${stamp}
        run: stamp-version
      - name: Synchronize stamped root lock entry
        run: |-
          ${syncRun.replace('\n', '\n          ')}
      - name: Build termul-server
        run: cargo build --locked --release --bin termul-server --features standalone-server
`
}

function parserAdapter(): string {
  return `
import { isConversationId as validConversationId } from '@shared/types/conversation.types'
export const valid = validConversationId(value)
`
}

function validSources(): Record<string, string> {
  return {
    '.github/workflows/pr-validation.yml': prWorkflow(),
    '.github/workflows/nightly.yml': packagingWorkflow('Stamp nightly version into sources'),
    '.github/workflows/release.yml': packagingWorkflow('Align app versions with tag'),
    '.github/workflows/other.yaml':
      'jobs:\n  check:\n    steps:\n      - run: cargo check --locked --all-targets\n',
    'src/renderer/App.tsx': root(),
    'src/renderer/TauriApp.tsx': root(),
    'src/renderer/app/PortableAppEffects.tsx': portableEffects(),
    'src/renderer/app/portable-router.tsx': portableRouter(),
    'src/renderer/pages/WorkspaceDashboard.tsx':
      'export default function WorkspaceDashboard() { return <main /> }',
    'src/renderer/lib/acp-history-persistence.ts': `
import { isConversationId } from '@shared/types/conversation.types'
import { acpHistoryApi as history } from '@/lib/acp-history-api'
const ok = isConversationId(id)
export async function page(mode: string) {
  if (mode === 'server') return transport.getSessionPayloadPage(id, 0, 250)
  return history.getPage(id, 0, 250)
}
`,
    'src/renderer/lib/conversation-lifecycle-api.ts': parserAdapter(),
    'src/renderer/lib/tauri-conversation-api.ts': parserAdapter(),
    'src/renderer/lib/tauri-session-workspace-api.ts': parserAdapter(),
    'src/renderer/lib/web-conversation-api.ts': parserAdapter(),
    'src/renderer/lib/web-session-workspace-api.ts': parserAdapter(),
    'src/renderer/lib/conversation-api.ts': `
import { sessionWorkspaceApi } from './session-workspace-api'
import { conversationLifecycleApi } from './conversation-lifecycle-api'
import { tauriConversationApi } from './tauri-conversation-api'
import { webConversationApi } from './web-conversation-api'
export const conversationApi = { sessionWorkspaceApi, conversationLifecycleApi, tauriConversationApi, webConversationApi }
`,
    'src/renderer/lib/acp-transport.ts': `
import { getRemoteAccessCredential as credential } from './remote-access-credential'
export const payload = { token: credential() }
`,
    'src/shared/types/session-workspace.types.ts': `
export interface SessionWorkspaceV1 { conversationId: string; revision: number }
`,
    'src/shared/types/web-terminal-protocol.types.ts': `
export interface TerminalSpawnIntentV1 { conversationId: string; cols: number; rows: number }
`
  }
}

function findings(sources: GuardSources, rule: string) {
  return checkConversationFirstGuardrails(sources).filter((item) => item.rule === rule)
}

const repositoryRoot = resolve(__dirname, '..')
const guardScript = resolve(__dirname, 'check-conversation-first-guardrails.ts')

function writeSources(root: string, sources: GuardSources): void {
  for (const [file, source] of Object.entries(sources)) {
    const target = join(root, file)
    mkdirSync(dirname(target), { recursive: true })
    writeFileSync(target, source)
  }
}

function withTemporaryRepository<T>(sources: GuardSources, run: (root: string) => T): T {
  const root = mkdtempSync(join(tmpdir(), 'conversation-first-guard-'))
  try {
    writeSources(root, sources)
    mkdirSync(join(root, 'scripts'), { recursive: true })
    symlinkSync(guardScript, join(root, 'scripts/check-conversation-first-guardrails.ts'))
    return run(root)
  } finally {
    rmSync(root, { force: true, recursive: true })
  }
}

function parseJsonFindings(stdout: string): GuardFinding[] {
  const parsed: unknown = JSON.parse(stdout)
  expect(Array.isArray(parsed)).toBe(true)
  return parsed as GuardFinding[]
}

describe('Conversation-first semantic guardrails', () => {
  it('accepts symbol-resolved aliases, executable JSX/calls, and structural workflows', () => {
    expect(checkConversationFirstGuardrails(validSources())).toEqual([])
  })

  it('ignores comments and string decoys but rejects a missing executable root node', () => {
    const sources = validSources()
    sources['src/renderer/App.tsx'] = root().replace(
      '<Effects />',
      '{/* <Effects /> */}{"<PortableAppEffects />"}'
    )
    const result = findings(sources, 'root-parity')
    expect(result.some((item) => item.file === 'src/renderer/App.tsx')).toBe(true)
  })

  it('rejects shadowed and unreachable root symbols even when the imported names remain', () => {
    const sources = validSources()
    sources['src/renderer/App.tsx'] = `
import { PortableAppEffects as Effects } from '@/app/PortableAppEffects'
import { createPortableRouter as buildRouter } from '@/app/portable-router'
import { ConversationHostStatus as Status } from '@/components/conversation/ConversationHostStatus'
import { ConversationRecoveryPanel as Recovery } from '@/components/conversation/ConversationRecoveryPanel'
function disconnected() { buildRouter(); return <Effects /> }
export default function Root() {
  const Effects = () => null
  if (false) buildRouter()
  return <><Effects /><Status /><Recovery /></>
}
`
    const result = findings(sources, 'root-parity').filter(
      (item) => item.file === 'src/renderer/App.tsx'
    )
    expect(result.map((item) => item.message)).toEqual(
      expect.arrayContaining([
        expect.stringContaining('execute the imported createPortableRouter'),
        expect.stringContaining('render the imported PortableAppEffects')
      ])
    )
    expect(result.every((item) => item.line > 0)).toBe(true)
  })

  it('requires reachable parser and history facade calls, not disconnected helpers', () => {
    const sources = validSources()
    sources['src/renderer/lib/conversation-lifecycle-api.ts'] = `
import { isConversationId } from '@shared/types/conversation.types'
function disconnected() { return isConversationId(value) }
export function live() { return true }
`
    sources['src/renderer/lib/acp-history-persistence.ts'] = `
import { isConversationId } from '@shared/types/conversation.types'
import { acpHistoryApi } from '@/lib/acp-history-api'
const ok = isConversationId(id)
function disconnected() { return acpHistoryApi.getPage(id, 0, 250) }
export function live() { return 'getSessionPayloadPage' }
`
    expect(findings(sources, 'shared-conversation-id-parser')).toEqual([
      expect.objectContaining({ file: 'src/renderer/lib/conversation-lifecycle-api.ts' })
    ])
    expect(findings(sources, 'history-paging-facade')).toHaveLength(2)
  })

  it('detects renamed teardown aliases through a reachable moved navigation helper', () => {
    const sources = validSources()
    sources['src/renderer/moved/navigation-owner.ts'] = `
import { terminalApi } from '@/lib/terminal-api'
const dispose = terminalApi.terminate
const stop = () => dispose('pty')
export function selectProject() { return stop() }
`
    const result = findings(sources, 'navigation-preserves-pty')
    expect(result).toHaveLength(1)
    expect(result[0]).toMatchObject({ file: 'src/renderer/moved/navigation-owner.ts' })
    expect(result[0].line).toBeGreaterThan(0)
  })

  it('ignores a disconnected teardown helper and comment/string decoys', () => {
    const sources = validSources()
    sources['src/renderer/moved/unreachable.ts'] = `
function disconnected() { return terminalApi.terminate('pty') }
// terminalApi.terminate('comment')
const decoy = "terminalApi.terminate('string')"
export function harmless() { return decoy.length }
`
    expect(findings(sources, 'navigation-preserves-pty')).toEqual([])
  })

  it('discovers new workflows and scans every Cargo occurrence after ||', () => {
    const sources = validSources()
    sources['.github/workflows/new-active.yaml'] = `
jobs:
  fresh:
    steps:
      - name: mixed
        run: cargo test --locked || cargo check --all-targets
`
    const result = findings(sources, 'locked-rust-ci')
    expect(result).toHaveLength(1)
    expect(result[0]).toMatchObject({ file: '.github/workflows/new-active.yaml' })
    expect(result[0].message).toContain('job=fresh')
    expect(result[0].message).toContain('cargo check')
    expect(result[0].line).toBeGreaterThan(0)
  })

  it('checks multiline and quoted Cargo commands structurally', () => {
    const sources = validSources()
    sources['.github/workflows/folded.yml'] = `
jobs:
  build:
    steps:
      - run: >
          sh -c "cargo build --release --bin termul-server"
`
    expect(findings(sources, 'locked-rust-ci')).toHaveLength(1)
  })

  it.each([
    [
      'comment-only decoy',
      `jobs:\n  validate:\n    steps:\n      # name: Check conversation-first guardrails\n      # run: bun run check:conversation-first\n`,
      'found 0'
    ],
    [
      'duplicate exact names',
      prWorkflow(
        `  duplicate:\n    steps:\n      - name: Check conversation-first guardrails\n        run: bun run check:conversation-first\n`
      ),
      'found 2'
    ],
    [
      'run suffix decoy',
      prWorkflow().replace(
        'run: bun run check:conversation-first',
        'run: bun run check:conversation-first && echo decoy'
      ),
      'run scalar must equal'
    ]
  ])('rejects default PR guard %s', (_name, workflow, evidence) => {
    const sources = validSources()
    sources['.github/workflows/pr-validation.yml'] = workflow
    const result = findings(sources, 'default-pr-guard')
    expect(result).toHaveLength(1)
    expect(result[0].message).toContain(evidence)
  })

  it.each([
    [
      'non-immediate sync',
      packagingWorkflow('Stamp nightly version into sources').replace(
        '      - name: Synchronize stamped root lock entry',
        '      - name: Intervening step\n        run: echo no\n      - name: Synchronize stamped root lock entry'
      ),
      'stamped-root-lock'
    ],
    [
      'extra sync command',
      packagingWorkflow('Stamp nightly version into sources').replace(
        'cargo metadata --locked --manifest-path src-tauri/Cargo.toml --format-version 1 --no-deps',
        'cargo metadata --locked --manifest-path src-tauri/Cargo.toml --format-version 1 --no-deps\n          echo extra'
      ),
      'stamped-root-lock'
    ],
    [
      'tauri suffix text',
      packagingWorkflow('Stamp nightly version into sources').replace(
        '-- --locked',
        '-- --locked suffix'
      ),
      'locked-tauri-action'
    ],
    [
      'tauri non-scalar args',
      packagingWorkflow('Stamp nightly version into sources').replace(
        'args: ${{ matrix.args }} --config src-tauri/tauri.conf.prod.json -- --locked',
        'args:\n            value: -- --locked'
      ),
      'locked-tauri-action'
    ]
  ])('rejects packaging workflow %s', (_name, workflow, rule) => {
    const sources = validSources()
    sources['.github/workflows/nightly.yml'] = workflow
    const result = findings(sources, rule)
    expect(result.length).toBeGreaterThan(0)
    expect(result[0].file).toBe('.github/workflows/nightly.yml')
    expect(result[0].line).toBeGreaterThan(0)
  })

  it('rejects dead short-circuit JSX root owner', () => {
    const sources = validSources()
    sources['src/renderer/App.tsx'] = root().replace('<Recovery />', '{false && <Recovery />}')
    const result = findings(sources, 'root-parity').filter(
      (item) => item.file === 'src/renderer/App.tsx'
    )
    expect(result).toHaveLength(1)
    expect(result[0].message).toContain('ConversationRecoveryPanel')
  })

  it('rejects destructured teardown alias without live call', () => {
    const sources = validSources()
    sources['src/renderer/moved/navigation-owner.ts'] = `
import { terminalApi } from '@/lib/terminal-api'
const { terminate: dispose } = terminalApi
export function selectProject() { return dispose('pty') }
`
    const result = findings(sources, 'navigation-preserves-pty')
    expect(result).toHaveLength(1)
    expect(result[0]).toMatchObject({ file: 'src/renderer/moved/navigation-owner.ts' })
    expect(result[0].message).toContain('terminate')
    expect(result[0].line).toBeGreaterThan(0)
  })

  it('rejects disabled workflow step as sole PR guard', () => {
    const sources = validSources()
    sources['.github/workflows/pr-validation.yml'] = prWorkflow().replace(
      '- name: Check conversation-first guardrails\n        run: bun run check:conversation-first',
      '- name: Check conversation-first guardrails\n        if: false\n        run: bun run check:conversation-first'
    )
    const result = findings(sources, 'default-pr-guard')
    expect(result).toHaveLength(1)
    expect(result[0].message).toContain('found 0')
  })

  it('rejects a dashboard-owned recovery panel in addition to the real root owner', () => {
    const sources = validSources()
    sources['src/renderer/pages/WorkspaceDashboard.tsx'] = `
import { ConversationRecoveryPanel } from '@/components/conversation/ConversationRecoveryPanel'
export default function WorkspaceDashboard() { return <ConversationRecoveryPanel /> }
`
    expect(findings(sources, 'recovery-owner')).toEqual([
      expect.objectContaining({ file: 'src/renderer/pages/WorkspaceDashboard.tsx' })
    ])
  })

  it('rejects raw remote spawn fields and executable placeholder credentials', () => {
    const sources = validSources()
    sources['src/shared/types/web-terminal-protocol.types.ts'] = `
export interface TerminalSpawnIntentV1 { conversationId: string; shell?: string }
`
    sources['src/renderer/lib/acp-transport.ts'] = `
function getRemoteAccessCredential() { return memory }
export const payload = { token: 'dev' }
`
    expect(findings(sources, 'remote-terminal-intent')).toHaveLength(1)
    expect(findings(sources, 'authenticated-remote-access')).toHaveLength(2)
  })

  it('retains process-compatible sanitized file:line/rule output', () => {
    const log = vi.spyOn(console, 'log').mockImplementation(() => undefined)
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    expect(main(validSources())).toBe(0)
    const sources = validSources()
    sources['.github/workflows/new-active.yaml'] =
      'jobs:\n  x:\n    steps:\n      - run: cargo test\n'
    expect(main(sources)).toBe(1)
    expect(error).toHaveBeenCalledWith(
      expect.stringMatching(/^\.github\/workflows\/new-active\.yaml:\d+ \[locked-rust-ci\]/)
    )
    log.mockRestore()
    error.mockRestore()
  })

  it('keeps sub-100 fixture scans in-process without spawning bun', () => {
    const originalPath = process.env.PATH
    process.env.PATH = '/conversation-first-no-bun'
    try {
      const sources = validSources()
      expect(Object.keys(sources).length).toBeLessThan(100)
      expect(checkConversationFirstGuardrails(sources)).toEqual([])
    } finally {
      process.env.PATH = originalPath
    }
  })

  it('memoizes by source identity in under 50ms without exposing cached mutable findings', () => {
    const sources = validSources()
    sources['.github/workflows/new-active.yaml'] =
      'jobs:\n  x:\n    steps:\n      - run: cargo test\n'
    const first = checkConversationFirstGuardrails(sources)
    expect(first).toHaveLength(1)
    first[0].message = 'mutated by caller'
    first.push({ rule: 'caller', file: 'caller', line: 1, message: 'caller' })

    const started = performance.now()
    const second = checkConversationFirstGuardrails(sources)
    const duration = performance.now() - started

    expect(duration).toBeLessThan(50)
    expect(second).toHaveLength(1)
    expect(second[0].message).not.toBe('mutated by caller')
    expect(second).not.toBe(first)
    second[0].message = 'mutated again'
    expect(checkConversationFirstGuardrails(sources)[0].message).not.toBe('mutated again')
  })

  it('filters repository loading to consumed sources while retaining exact rules and decoys', () => {
    const sources = validSources()
    sources['src/renderer/irrelevant.ts'] = 'export const harmless = true\n'
    sources['src/renderer/irrelevant.test.ts'] = 'terminalApi.terminate("test-only")\n'
    sources['src/renderer/moved/navigation-owner.ts'] = `
import { terminalApi } from '@/lib/terminal-api'
const dispose = terminalApi.terminate
export function selectProject() { return dispose('pty') }
`
    sources['src/renderer/moved/string-decoy.ts'] = `
const decoy = "terminalApi.terminate('string')"
export function harmless() { return decoy.length }
`

    withTemporaryRepository(sources, (root) => {
      const loaded = loadRepositorySources(root)
      const loadedFiles = Object.keys(loaded)
      expect(loadedFiles).not.toContain('src/renderer/irrelevant.ts')
      expect(loadedFiles).not.toContain('src/renderer/irrelevant.test.ts')
      expect(loadedFiles).toContain('src/renderer/moved/navigation-owner.ts')
      expect(loadedFiles).toContain('src/renderer/moved/string-decoy.ts')
      expect(loadedFiles).toEqual(
        expect.arrayContaining(
          Object.keys(validSources()).filter((file) => !file.endsWith('.test.ts'))
        )
      )
      const repositoryMarker = Object.getOwnPropertySymbols(loaded)
      expect(repositoryMarker).toHaveLength(1)
      expect(Object.getOwnPropertyDescriptor(loaded, repositoryMarker[0])?.enumerable).toBe(false)

      const inProcess = checkConversationFirstGuardrails(sources)
      const delegated = checkConversationFirstGuardrails(loaded)
      expect(delegated).toEqual(inProcess)
      expect(delegated.filter((finding) => finding.rule === 'navigation-preserves-pty')).toEqual([
        expect.objectContaining({ file: 'src/renderer/moved/navigation-owner.ts' })
      ])
      expect(
        delegated.some((finding) => finding.file === 'src/renderer/moved/string-decoy.ts')
      ).toBe(false)
    })
  })

  it('emits only GuardFinding[] JSON and uses finding-sensitive CLI exit status', () => {
    const sources = validSources()
    sources['.github/workflows/new-active.yaml'] =
      'jobs:\n  x:\n    steps:\n      - run: cargo test\n'
    withTemporaryRepository(sources, (root) => {
      const result = spawnSync('bun', [guardScript, '--json'], {
        cwd: root,
        encoding: 'utf8',
        env: { ...process.env, PATH: `${process.env.HOME}/.bun/bin:${process.env.PATH}` }
      })
      const parsed = parseJsonFindings(result.stdout)
      expect(result.status).toBe(1)
      expect(result.stderr).toBe('')
      expect(parsed).toHaveLength(1)
      expect(parsed[0]).toMatchObject({
        file: '.github/workflows/new-active.yaml',
        rule: 'locked-rust-ci'
      })
    })
  })

  it('keeps real Bun and Node-delegated scans equal and within 4000ms', () => {
    const bunStarted = performance.now()
    const bunResult = spawnSync(
      'bun',
      ['scripts/check-conversation-first-guardrails.ts', '--json'],
      {
        cwd: repositoryRoot,
        encoding: 'utf8',
        env: { ...process.env, PATH: `${process.env.HOME}/.bun/bin:${process.env.PATH}` }
      }
    )
    const bunDuration = performance.now() - bunStarted
    const bunFindings = parseJsonFindings(bunResult.stdout)
    expect(bunResult.status).toBe(0)
    expect(bunResult.stderr).toBe('')
    expect(bunDuration).toBeLessThan(4000)

    const sources = loadRepositorySources(repositoryRoot)
    const nodeStarted = performance.now()
    const nodeFindings = checkConversationFirstGuardrails(sources)
    const nodeDuration = performance.now() - nodeStarted
    expect(nodeDuration).toBeLessThan(4000)
    expect(nodeFindings).toEqual(bunFindings)
  })

  it('keeps compatibility stripping helpers line-stable', () => {
    const source = `const live = true\n// kill_all()\n/* terminate() */\nconst end = true`
    const stripped = stripComments(source)
    expect(stripped).not.toContain('kill_all')
    expect(stripped).not.toContain('terminate')
    expect(stripped.split('\n')).toHaveLength(source.split('\n').length)

    const rust = `fn live() {}\n#[cfg(test)]\nmod tests { fn kill_all() {} }`
    const production = stripRustTestCode(rust)
    expect(production).toContain('fn live() {}')
    expect(production).not.toContain('kill_all')
    expect(production.split('\n')).toHaveLength(rust.split('\n').length)
  })
})
