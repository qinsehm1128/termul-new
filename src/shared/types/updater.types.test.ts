import { describe, expect, it } from 'vitest'
import { legacyUpdateComponentPolicy, parseUpdateComponentPolicy } from './updater.types'

describe('update component policy', () => {
  it('creates a conservative legacy policy when metadata is absent', () => {
    const policy = parseUpdateComponentPolicy(undefined, '1.2.3')

    expect(policy).toEqual(legacyUpdateComponentPolicy('1.2.3'))
    expect(policy.metadataState).toBe('legacy')
    expect(policy.components.terminalCore.action).toBe('defer-if-active')
    expect(policy.components.pluginCore).toEqual({
      buildId: 'legacy:1.2.3:pluginCore',
      action: 'unsupported'
    })
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
    expect(policy.components.pluginCore).toEqual({
      buildId: 'legacy:1.2.3:pluginCore',
      action: 'unsupported'
    })
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

const fourKeyPolicy = {
  schemaVersion: 1,
  targetVersion: '1.2.3',
  metadataState: 'declared',
  components: {
    renderer: { buildId: 'renderer-1', action: 'restart' },
    guiNative: { buildId: 'gui-1', action: 'restart' },
    acpCore: { buildId: 'acp-1', action: 'preserve' },
    terminalCore: { buildId: 'terminal-1', action: 'defer-if-active' }
  }
}
const pluginEntry = {
  buildId: `termul-pluginCore-v1-sha256:${'a'.repeat(64)}`,
  action: 'defer-if-active'
}

describe('Plugin Core wire compatibility', () => {
  it('normalizes the public sibling extension and accepts the persisted five-key shape', () => {
    const normalized = parseUpdateComponentPolicy(
      { ...fourKeyPolicy, pluginCore: pluginEntry },
      '1.2.3'
    )
    expect(Object.keys(normalized.components)).toHaveLength(5)
    expect(normalized.components.pluginCore).toEqual(pluginEntry)
    expect(parseUpdateComponentPolicy(normalized, '1.2.3')).toEqual(normalized)
  })

  it('allows matching duplicates but rejects either kind of conflicting duplicate', () => {
    const five = {
      ...fourKeyPolicy,
      components: { ...fourKeyPolicy.components, pluginCore: pluginEntry }
    }
    expect(
      parseUpdateComponentPolicy({ ...five, pluginCore: pluginEntry }, '1.2.3').components
        .pluginCore
    ).toEqual(pluginEntry)
    for (const conflicting of [
      { ...pluginEntry, buildId: 'different' },
      { ...pluginEntry, action: 'restart' }
    ]) {
      expect(() =>
        parseUpdateComponentPolicy({ ...five, pluginCore: conflicting }, '1.2.3')
      ).toThrow('conflicting pluginCore')
    }
  })

  it('still rejects missing, unknown, and malformed component entries', () => {
    const { acpCore: _acp, ...incomplete } = fourKeyPolicy.components
    for (const components of [incomplete, { ...fourKeyPolicy.components, typoCore: pluginEntry }]) {
      expect(() => parseUpdateComponentPolicy({ ...fourKeyPolicy, components }, '1.2.3')).toThrow(
        'incomplete or contain unknown'
      )
    }
    for (const pluginCore of [
      null,
      undefined,
      {},
      { buildId: ' ', action: 'restart' },
      { buildId: 'id', action: 'unknown' }
    ]) {
      expect(() => parseUpdateComponentPolicy({ ...fourKeyPolicy, pluginCore }, '1.2.3')).toThrow(
        'pluginCore'
      )
    }
  })
})
