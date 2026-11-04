import type { PluginInfo, PluginService } from '@shared/types/plugin.types'
import { cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { PluginServices } from './PluginServices'

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
afterEach(cleanup)

function fixture(
  id = 'org.plugin',
  provides: PluginService[] = [],
  requires: PluginService[] = []
): PluginInfo {
  return {
    id,
    desiredEnabled: true,
    state: 'active',
    generation: 'g1',
    configRevision: 1,
    sha256: '',
    lastError: null,
    manifest: {
      schemaVersion: 1,
      id,
      name: id,
      version: '1.0.0',
      abi: { min: 1, max: 1 },
      contributes: { commands: [], views: [], configuration: { title: '', properties: {} } },
      services: { provides, requires }
    }
  }
}

function serviceRow(name: string): HTMLElement {
  const row = screen.getByText(name).closest('li')
  if (!row) throw new Error(`Missing service row: ${name}`)
  return row
}

describe('PluginServices', () => {
  it('shows provided names and versions and availability only while active and connected', () => {
    const plugin = fixture('org.plugin', [{ name: 'org.service', version: 3 }])
    const { rerender } = render(<PluginServices plugin={plugin} plugins={[]} connected />)
    expect(within(serviceRow('org.service')).getByText('v3')).toBeTruthy()
    expect(within(serviceRow('org.service')).getByText('inspector.available')).toBeTruthy()
    rerender(<PluginServices plugin={plugin} plugins={[]} connected={false} />)
    expect(within(serviceRow('org.service')).getByText('inspector.unavailable')).toBeTruthy()
    rerender(<PluginServices plugin={{ ...plugin, state: 'disabled' }} plugins={[]} connected />)
    expect(within(serviceRow('org.service')).getByText('inspector.unavailable')).toBeTruthy()
    rerender(
      <PluginServices plugin={{ ...plugin, desiredEnabled: false }} plugins={[]} connected />
    )
    expect(within(serviceRow('org.service')).getByText('inspector.unavailable')).toBeTruthy()
  })

  it('requires exact name and version from active enabled peers, not self or inactive providers', () => {
    const requires = [
      { name: 'org.match', version: 2 },
      { name: 'org.wrong-version', version: 2 },
      { name: 'org.disabled', version: 1 },
      { name: 'org.failed', version: 1 },
      { name: 'org.self', version: 1 },
      { name: 'org.absent', version: 1 }
    ]
    const plugin = fixture('org.plugin', [{ name: 'org.self', version: 1 }], requires)
    const peers = [
      plugin,
      fixture('org.active', [
        { name: 'org.match', version: 2 },
        { name: 'org.wrong-version', version: 1 }
      ]),
      {
        ...fixture('org.disabled-peer', [{ name: 'org.disabled', version: 1 }]),
        desiredEnabled: false
      },
      {
        ...fixture('org.failed-peer', [{ name: 'org.failed', version: 1 }]),
        state: 'failed' as const
      }
    ]
    const { rerender } = render(<PluginServices plugin={plugin} plugins={peers} connected />)
    const required = within(screen.getByRole('region', { name: 'inspector.requires' }))
    expect(required.getAllByText('inspector.available')).toHaveLength(1)
    expect(required.getAllByText('inspector.unavailable')).toHaveLength(5)
    expect(within(serviceRow('org.match')).getByText('inspector.available')).toBeTruthy()
    rerender(<PluginServices plugin={plugin} plugins={peers} connected={false} />)
    expect(required.queryByText('inspector.available')).toBeNull()
  })

  it('uses the host availability allowlist and cannot be fooled by a reserved peer provider', () => {
    const plugin = fixture(
      'org.plugin',
      [],
      [
        { name: 'se.host.context', version: 1 },
        { name: 'se.host.storage', version: 2 },
        { name: 'se.host.secrets', version: 1 },
        { name: 'se.host.unknown', version: 1 }
      ]
    )
    const spoof = fixture('org.spoof', [
      { name: 'se.host.secrets', version: 1 },
      { name: 'se.host.unknown', version: 1 },
      { name: 'se.host.storage', version: 2 }
    ])
    render(<PluginServices plugin={plugin} plugins={[spoof]} connected />)
    expect(within(serviceRow('se.host.context')).getByText('inspector.available')).toBeTruthy()
    for (const name of ['se.host.storage', 'se.host.secrets', 'se.host.unknown']) {
      expect(within(serviceRow(name)).getByText('inspector.unavailable')).toBeTruthy()
    }
    expect(screen.getByText('inspector.secretsUnavailable')).toBeTruthy()
  })

  it('always shows the secrets unavailable notice and both empty states without requirements', () => {
    render(<PluginServices plugin={fixture()} plugins={[]} connected />)
    expect(screen.getByText('inspector.emptyProvides')).toBeTruthy()
    expect(screen.getByText('inspector.emptyRequires')).toBeTruthy()
    expect(screen.getByText('inspector.secretsUnavailable')).toBeTruthy()
  })
})
