import { emptyMcpHostAccessPolicy } from '@shared/types/mcp-host.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import i18next from 'i18next'
import { I18nextProvider, initReactI18next } from 'react-i18next'
import { afterEach, expect, it, vi } from 'vitest'
import en from '../../locales/en/mcp.json'
import { McpHostAccessPanel } from './McpHostAccessPanel'

const { get, put, call } = vi.hoisted(() => ({ get: vi.fn(), put: vi.fn(), call: vi.fn() }))
vi.mock('@/lib/mcp-host-api', () => ({
  getMcpHostAccess: get,
  putMcpHostAccess: put,
  callMcpHost: call
}))
afterEach(cleanup)

it('preserves project and SSH drafts with the actual translation provider without refetching', async () => {
  const i18n = i18next.createInstance()
  await i18n.use(initReactI18next).init({
    lng: 'en',
    resources: { en: { mcp: en } },
    defaultNS: 'mcp',
    interpolation: { escapeValue: false }
  })
  get.mockImplementation(async () => ({
    success: true,
    data: {
      policy: emptyMcpHostAccessPolicy(),
      profiles: [
        {
          profileId: 'p',
          name: 'Production',
          host: 'fixture.invalid',
          port: 22,
          username: 'operator',
          authMethod: 'key',
          tags: [],
          fingerprint: 'pin'
        }
      ],
      projects: [
        { id: 'a', name: 'Alpha' },
        { id: 'b', name: 'Beta' }
      ],
      hostMode: 'LOCAL_OPERATOR'
    }
  }))
  render(
    <I18nextProvider i18n={i18n}>
      <McpHostAccessPanel />
    </I18nextProvider>
  )
  await screen.findByText('Production')
  const grant = screen.getByRole('checkbox', { name: en.hostAccess.grantProfile })
  fireEvent.click(grant)
  fireEvent.change(screen.getByRole('searchbox', { name: en.hostAccess.searchProjects }), {
    target: { value: 'Alpha' }
  })
  fireEvent.click(screen.getByRole('button', { name: en.hostAccess.selectResults }))
  fireEvent.click(screen.getByRole('button', { name: en.hostAccess.selectReadOnly }))
  fireEvent.click(screen.getByRole('button', { name: en.hostAccess.collapseAll }))
  await act(async () => {
    await Promise.resolve()
  })
  await waitFor(() => expect(screen.getByRole('checkbox', { name: 'Alpha' })).toBeChecked())
  expect(grant).toBeChecked()
  expect(screen.getByRole('button', { name: 'Configure Production' })).toHaveAttribute(
    'aria-expanded',
    'false'
  )
  expect(screen.getByText(en.hostAccess.unsaved)).toBeInTheDocument()
  expect(get).toHaveBeenCalledTimes(1)
  expect(put).not.toHaveBeenCalled()
  expect(call).not.toHaveBeenCalled()
})
