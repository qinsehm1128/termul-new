import { emptyMcpHostAccessPolicy } from '@shared/types/mcp-host.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import i18next from 'i18next'
import { I18nextProvider, initReactI18next } from 'react-i18next'
import { afterEach, expect, it, vi } from 'vitest'
import en from '../../locales/en/mcp.json'
import zh from '../../locales/zh-CN/mcp.json'
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
    resources: { en: { mcp: en }, 'zh-CN': { mcp: zh } },
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
  await screen.findByRole('button', { name: 'Configure Production' })
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
  for (const key of ['help', 'previewHelp'] as const) {
    expect(screen.getAllByText(en.commandDeny[key]).length).toBeGreaterThan(0)
    expect(en.commandDeny[key]).toContain(
      'Manual local terminals and interactive SSH terminals are not restricted'
    )
    expect(en.commandDeny[key]).toContain('execute')
    expect(en.commandDeny[key]).toContain('job_start')
  }
  const globalPattern = screen.getAllByLabelText(en.commandDeny.newPattern)[0]
  fireEvent.change(globalPattern, { target: { value: 'literal-danger' } })
  fireEvent.click(screen.getAllByRole('button', { name: en.commandDeny.add })[0])
  await act(async () => {
    await i18n.changeLanguage('zh-CN')
  })
  expect(screen.getByText(zh.commandDeny.previewTitle)).toBeInTheDocument()
  for (const key of ['help', 'previewHelp'] as const) {
    expect(screen.getAllByText(zh.commandDeny[key]).length).toBeGreaterThan(0)
    expect(zh.commandDeny[key]).toContain('不限制手动本地终端或 SSH 交互终端操作')
    expect(zh.commandDeny[key]).toContain('execute')
    expect(zh.commandDeny[key]).toContain('job_start')
  }
  expect(screen.getAllByDisplayValue('literal-danger')[0]).toHaveAttribute(
    'aria-label',
    expect.stringContaining('规则模式')
  )
  expect(get).toHaveBeenCalledTimes(1)
  expect(put).not.toHaveBeenCalled()
  expect(call).not.toHaveBeenCalled()
})
