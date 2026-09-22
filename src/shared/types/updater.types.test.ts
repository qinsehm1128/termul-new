import { describe, expect, it } from 'vitest'
import { legacyUpdateComponentPolicy, parseUpdateComponentPolicy } from './updater.types'

describe('update component policy', () => {
  it('creates a conservative legacy policy when metadata is absent', () => {
    const policy = parseUpdateComponentPolicy(undefined, '1.2.3')

    expect(policy).toEqual(legacyUpdateComponentPolicy('1.2.3'))
    expect(policy.metadataState).toBe('legacy')
    expect(policy.components.terminalCore.action).toBe('defer-if-active')
  })

  it('accepts a declared policy for the matching target version', () => {
    const policy = parseUpdateComponentPolicy(
      {
        schemaVersion: 1,
        targetVersion: '1.2.3',
        metadataState: 'declared',
        components: {
          renderer: { buildId: 'renderer-1', action: 'restart' },
          guiNative: { buildId: 'gui-1', action: 'restart' },
          acpCore: { buildId: 'acp-1', action: 'preserve' },
          terminalCore: { buildId: 'terminal-1', action: 'defer-if-active' }
        }
      },
      '1.2.3'
    )

    expect(policy.metadataState).toBe('declared')
    expect(policy.components.acpCore.buildId).toBe('acp-1')
  })

  it('rejects a policy that targets another version', () => {
    expect(() =>
      parseUpdateComponentPolicy(
        {
          schemaVersion: 1,
          targetVersion: '9.9.9',
          metadataState: 'declared',
          components: {}
        },
        '1.2.3'
      )
    ).toThrow('targetVersion')
  })
})
