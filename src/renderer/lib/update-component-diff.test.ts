import { parseUpdateComponentPolicy, type UpdateComponentPolicy } from '@shared/types/updater.types'
import { describe, expect, it } from 'vitest'
import { forceComponentPolicy } from './tauri-update-plan-api'
import {
  buildComponentDiff,
  type ComponentRuntimeIdentities,
  forcedUpdateCasualties,
  shortBuildId
} from './update-component-diff'

const HASH = 'a'.repeat(52) + '0123456789ab'

const policy: UpdateComponentPolicy = {
  schemaVersion: 1,
  targetVersion: '0.14.9',
  metadataState: 'declared',
  components: {
    renderer: { buildId: 'renderer-next', action: 'restart' },
    guiNative: { buildId: 'gui-next', action: 'restart' },
    acpCore: { buildId: 'acp-next', action: 'defer-if-active' },
    terminalCore: { buildId: 'terminal-same', action: 'defer-if-active' }
  }
}

const runtime: ComponentRuntimeIdentities = {
  guiNative: 'gui-old',
  acpCore: { buildId: 'acp-old', activeResources: 2 },
  terminalCore: { buildId: 'terminal-same', activeResources: 5 }
}

describe('buildComponentDiff', () => {
  it('compares each running build ID with the release', () => {
    const rows = buildComponentDiff(policy, runtime)

    expect(rows.map((row) => [row.component, row.status])).toEqual([
      ['renderer', 'bundled'],
      ['guiNative', 'replace'],
      ['acpCore', 'replace'],
      ['terminalCore', 'same']
    ])
    expect(rows[2]).toMatchObject({
      currentBuildId: 'acp-old',
      nextBuildId: 'acp-next',
      activeResources: 2
    })
  })

  it('marks a Core that is not listening as not running', () => {
    const rows = buildComponentDiff(policy, { ...runtime, terminalCore: null })

    expect(rows[3]).toMatchObject({ status: 'notRunning', currentBuildId: null })
  })

  it('treats a Core that reports no build ID as replaced', () => {
    const rows = buildComponentDiff(policy, {
      ...runtime,
      acpCore: { buildId: null, activeResources: 0 }
    })

    expect(rows[2].status).toBe('replace')
  })
})

describe('forcedUpdateCasualties', () => {
  it('counts only the work of Cores that are replaced', () => {
    expect(forcedUpdateCasualties(buildComponentDiff(policy, runtime))).toEqual({
      terminals: 0,
      agentSessions: 2
    })
    expect(
      forcedUpdateCasualties(
        buildComponentDiff(policy, {
          ...runtime,
          terminalCore: { buildId: 'terminal-old', activeResources: 5 }
        })
      )
    ).toEqual({ terminals: 5, agentSessions: 2 })
  })
})

describe('shortBuildId', () => {
  it('shows the last 12 digest characters of a release build ID', () => {
    expect(shortBuildId(`termul-acpCore-v1-sha256:${HASH}`)).toBe('0123456789ab')
  })

  it('shows any other build ID unchanged', () => {
    expect(shortBuildId('0.14.9:terminal-core')).toBe('0.14.9:terminal-core')
  })
})

describe('forceComponentPolicy', () => {
  it('turns deferred Cores into restarts and keeps everything else', () => {
    const forced = forceComponentPolicy(policy)

    expect(forced.components.acpCore).toEqual({ buildId: 'acp-next', action: 'restart' })
    expect(forced.components.terminalCore).toEqual({
      buildId: 'terminal-same',
      action: 'restart'
    })
    expect(forced.components.renderer).toBe(policy.components.renderer)
    expect(forced.components.guiNative).toBe(policy.components.guiNative)
    expect(policy.components.acpCore.action).toBe('defer-if-active')
  })

  it('leaves preserve and unsupported Cores alone', () => {
    const forced = forceComponentPolicy({
      ...policy,
      components: {
        ...policy.components,
        acpCore: { buildId: 'acp-next', action: 'preserve' },
        terminalCore: { buildId: 'terminal-next', action: 'unsupported' }
      }
    })

    expect(forced.components.acpCore.action).toBe('preserve')
    expect(forced.components.terminalCore.action).toBe('unsupported')
  })

  it('still parses as a published component policy', () => {
    const forced = forceComponentPolicy(policy)

    expect(parseUpdateComponentPolicy(forced, '0.14.9')).toEqual(forced)
  })
})
