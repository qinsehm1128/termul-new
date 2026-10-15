import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

const runtime = vi.hoisted(() => ({ desktop: true }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
vi.mock('@/stores/mcp-store', () => ({
  useMcpStore: (select: (state: { load: () => Promise<void> }) => unknown) =>
    select({ load: () => Promise.resolve() })
}))
vi.mock('@/components/settings/McpGatewayPanel', () => ({ McpGatewayPanel: () => <p>gateway</p> }))
vi.mock('@/components/settings/McpControlPanel', () => ({ McpControlPanel: () => <p>servers</p> }))
vi.mock('@/components/settings/McpDescriptionsPanel', () => ({
  McpDescriptionsPanel: () => <p>descriptions</p>
}))

import McpPage from './McpPage'

afterEach(() => cleanup())

function tabNames(): string[] {
  return screen.getAllByRole('tab').map((tab) => tab.textContent ?? '')
}

describe('McpPage', () => {
  it('splits the desktop page into tabs, opening on the agent connection', () => {
    runtime.desktop = true
    render(<McpPage />)
    expect(tabNames()).toEqual(['Connect agents', 'MCP servers', 'Server descriptions'])
    expect(
      screen.getByText('gateway').closest('[role="tabpanel"]')?.getAttribute('data-state')
    ).toBe('active')
  })

  it('offers only the servers on the web, where the gateway does not run', () => {
    runtime.desktop = false
    render(<McpPage />)
    expect(tabNames()).toEqual(['MCP servers'])
    expect(screen.queryByText('gateway')).toBeNull()
    expect(
      screen.getByText('servers').closest('[role="tabpanel"]')?.getAttribute('data-state')
    ).toBe('active')
  })
})
