import type { IpcResult } from '@shared/types/ipc.types'
import type { ConfigSnapshot } from '@shared/types/plugin.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PluginSettingsForm } from './PluginSettingsForm'
import { pluginApi } from './plugin-api'

vi.mock('./plugin-api', () => ({
  pluginApi: { getConfig: vi.fn(), setConfig: vi.fn() }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn().mockResolvedValue(undefined) }))
vi.mock('react-i18next', () => ({
  useTranslation: (namespace: string) => {
    expect(namespace).toBe('plugins')
    return {
      t: (key: string) => {
        expect(key).toMatch(/^(?:settings\.|view\.|state\.|errors\.generic$)/)
        return key
      }
    }
  }
}))

const defaults = {
  greeting: 'Hello',
  step: 1,
  showCount: true,
  tone: 'normal',
  size: 2,
  visible: true
}

function snapshot(revision = 7): ConfigSnapshot {
  return {
    pluginId: 'org.termul.example',
    revision,
    schema: {
      title: 'Counter settings',
      properties: {
        greeting: {
          type: 'string',
          title: 'Greeting',
          description: 'Greeting shown in panel',
          default: 'Hello',
          minLength: 1,
          maxLength: 12
        },
        step: {
          type: 'number',
          title: 'Step',
          description: 'Increment per click',
          default: 1,
          minimum: 1,
          maximum: 100
        },
        showCount: {
          type: 'boolean',
          title: 'Show count',
          description: 'Show current count',
          default: true
        },
        tone: {
          type: 'string',
          title: 'Tone',
          description: 'Text emphasis',
          default: 'normal',
          enum: ['normal', 'muted'],
          enumLabels: ['Normal', 'Muted']
        },
        size: {
          type: 'number',
          title: 'Size',
          description: 'Numeric enum',
          default: 2,
          enum: [2, 4],
          enumLabels: ['Small', 'Large']
        },
        visible: {
          type: 'boolean',
          title: 'Visible',
          description: 'Boolean enum',
          default: true,
          enum: [true, false],
          enumLabels: ['Visible', 'Hidden']
        }
      }
    },
    values: { ...defaults, greeting: 'Hi', step: 3, showCount: false },
    overrides: { greeting: 'Hi', step: 3, showCount: false }
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

async function ready() {
  render(<PluginSettingsForm pluginId="org.termul.example" />)
  await screen.findByRole('textbox', { name: 'Greeting' })
}

async function choose(label: string, option: string) {
  fireEvent.keyDown(screen.getByRole('combobox', { name: label }), { key: 'ArrowDown' })
  fireEvent.click(await screen.findByRole('option', { name: option }))
}

beforeEach(() => {
  vi.resetAllMocks()
  vi.mocked(pluginApi.getConfig).mockResolvedValue({ success: true, data: snapshot() })
  vi.mocked(pluginApi.setConfig).mockResolvedValue({ success: true, data: snapshot(8) })
  // Radix Select scrolls the focused item; jsdom has no scrolling implementation.
  Element.prototype.scrollIntoView = vi.fn()
})
afterEach(cleanup)

describe('PluginSettingsForm', () => {
  it('loads schema titles, descriptions, defaults and all four themed controls', async () => {
    await ready()
    expect(pluginApi.getConfig).toHaveBeenCalledWith({ id: 'org.termul.example' })
    expect(screen.getByRole('heading', { name: 'Counter settings' })).toBeInTheDocument()
    expect(screen.getByText('Greeting shown in panel')).toBeInTheDocument()
    expect(screen.getByText('"Hello"')).toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: 'Greeting' })).toHaveValue('Hi')
    expect(screen.getByRole('spinbutton', { name: 'Step' })).toHaveValue(3)
    expect(screen.getByRole('switch', { name: 'Show count' })).not.toBeChecked()
    expect(screen.getByRole('combobox', { name: 'Tone' })).toHaveTextContent('Normal')
    expect(screen.getByRole('combobox', { name: 'Size' })).toHaveTextContent('Small')
    expect(screen.getByRole('combobox', { name: 'Visible' })).toHaveTextContent('Visible')
  })

  it('saves complete typed values, maps enumLabels, and uses the new revision next time', async () => {
    await ready()
    fireEvent.change(screen.getByLabelText('Greeting'), { target: { value: 'Welcome' } })
    fireEvent.change(screen.getByLabelText('Step'), { target: { value: '2.5' } })
    fireEvent.click(screen.getByRole('switch', { name: 'Show count' }))
    await choose('Tone', 'Muted')
    await choose('Size', 'Large')
    await choose('Visible', 'Hidden')
    const updated = snapshot(8)
    updated.values = {
      greeting: 'Welcome',
      step: 2.5,
      showCount: true,
      tone: 'muted',
      size: 4,
      visible: false
    }
    vi.mocked(pluginApi.setConfig).mockResolvedValue({ success: true, data: updated })
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    await waitFor(() =>
      expect(pluginApi.setConfig).toHaveBeenCalledWith({
        id: 'org.termul.example',
        expectedRevision: 7,
        values: updated.values
      })
    )
    await waitFor(() => expect(screen.getByRole('button', { name: 'settings.save' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    await waitFor(() =>
      expect(pluginApi.setConfig).toHaveBeenLastCalledWith({
        id: 'org.termul.example',
        expectedRevision: 8,
        values: updated.values
      })
    )
  })

  it.each([
    '',
    'NaN',
    'Infinity',
    '1e',
    '0',
    '101'
  ])('does not send invalid number %j', async (value) => {
    await ready()
    fireEvent.change(screen.getByLabelText('Step'), { target: { value } })
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('settings.invalid')
    expect(pluginApi.setConfig).not.toHaveBeenCalled()
  })

  it('rejects an enum value outside the schema without sending', async () => {
    const invalid = snapshot()
    invalid.values.tone = 'unknown'
    vi.mocked(pluginApi.getConfig).mockResolvedValue({ success: true, data: invalid })
    await ready()
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('settings.invalid')
    expect(pluginApi.setConfig).not.toHaveBeenCalled()
  })

  it('applies shared string constraints without sending', async () => {
    await ready()
    fireEvent.change(screen.getByLabelText('Greeting'), { target: { value: '' } })
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('settings.invalid')
    expect(pluginApi.setConfig).not.toHaveBeenCalled()
  })

  it('Reset sends all manifest defaults even when the draft is invalid', async () => {
    await ready()
    fireEvent.change(screen.getByLabelText('Step'), { target: { value: '' } })
    const reset = snapshot(8)
    reset.values = { ...defaults }
    vi.mocked(pluginApi.setConfig).mockResolvedValue({ success: true, data: reset })
    fireEvent.click(screen.getByRole('button', { name: 'settings.reset' }))
    await waitFor(() =>
      expect(pluginApi.setConfig).toHaveBeenCalledWith({
        id: 'org.termul.example',
        expectedRevision: 7,
        values: defaults
      })
    )
    await waitFor(() => expect(screen.getByLabelText('Greeting')).toHaveValue('Hello'))
    expect(screen.getByLabelText('Step')).toHaveValue(1)
  })

  it('disables controls and both buttons until saving finishes', async () => {
    const pending = deferred<IpcResult<ConfigSnapshot>>()
    vi.mocked(pluginApi.setConfig).mockReturnValue(pending.promise)
    await ready()
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    expect(screen.getByRole('button', { name: 'settings.saving' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'settings.reset' })).toBeDisabled()
    expect(screen.getByLabelText('Greeting')).toBeDisabled()
    expect(screen.getByRole('switch', { name: 'Show count' })).toBeDisabled()
    expect(screen.getByRole('combobox', { name: 'Tone' })).toBeDisabled()
    fireEvent.submit(screen.getByRole('form'))
    expect(pluginApi.setConfig).toHaveBeenCalledTimes(1)
    await act(async () => pending.resolve({ success: true, data: snapshot(8) }))
    expect(screen.getByRole('button', { name: 'settings.save' })).toBeEnabled()
  })

  it('keeps a conflict draft until explicit reload and then uses the latest revision', async () => {
    vi.mocked(pluginApi.setConfig).mockResolvedValue({
      success: false,
      code: 'PLUGIN_CONFIG_CONFLICT',
      error: 'conflict'
    })
    await ready()
    fireEvent.change(screen.getByLabelText('Greeting'), { target: { value: 'My draft' } })
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('settings.conflict')
    expect(screen.getByLabelText('Greeting')).toHaveValue('My draft')
    expect(pluginApi.getConfig).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('button', { name: 'settings.save' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'settings.reset' })).toBeDisabled()
    const latest = snapshot(20)
    latest.values.greeting = 'Server'
    vi.mocked(pluginApi.getConfig).mockResolvedValue({ success: true, data: latest })
    fireEvent.click(screen.getByRole('button', { name: 'settings.reload' }))
    await waitFor(() => expect(screen.getByLabelText('Greeting')).toHaveValue('Server'))
    vi.mocked(pluginApi.setConfig).mockResolvedValue({ success: true, data: snapshot(21) })
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    await waitFor(() =>
      expect(pluginApi.setConfig).toHaveBeenLastCalledWith({
        id: 'org.termul.example',
        expectedRevision: 20,
        values: latest.values
      })
    )
  })

  it('shows API load errors and supports retry', async () => {
    vi.mocked(pluginApi.getConfig).mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_NOT_SUPPORTED',
      error: 'desktop only'
    })
    render(<PluginSettingsForm pluginId="org.termul.example" />)
    expect(await screen.findByRole('alert')).toHaveTextContent('errors.generic')
    expect(screen.queryByRole('button', { name: 'settings.save' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'settings.reload' }))
    expect(await screen.findByLabelText('Greeting')).toHaveValue('Hi')
  })

  it('keeps the draft and re-enables saving after a rejected request', async () => {
    vi.mocked(pluginApi.setConfig).mockRejectedValueOnce(new Error('transport unavailable'))
    await ready()
    fireEvent.change(screen.getByLabelText('Greeting'), { target: { value: 'Draft' } })
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('errors.generic')
    expect(screen.getByLabelText('Greeting')).toHaveValue('Draft')
    expect(screen.getByRole('button', { name: 'settings.save' })).toBeEnabled()
  })

  it('renders an empty configuration without save/reset controls', async () => {
    const empty = snapshot()
    empty.schema.properties = {}
    empty.values = {}
    vi.mocked(pluginApi.getConfig).mockResolvedValue({ success: true, data: empty })
    render(<PluginSettingsForm pluginId="org.termul.example" />)
    expect(await screen.findByText('settings.empty')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'settings.save' })).not.toBeInTheDocument()
  })

  it('ignores a stale load after changing pluginId', async () => {
    const old = deferred<IpcResult<ConfigSnapshot>>()
    vi.mocked(pluginApi.getConfig).mockReturnValueOnce(old.promise)
    const { rerender } = render(<PluginSettingsForm pluginId="org.termul.old" />)
    expect(screen.getByRole('status')).toHaveTextContent('view.loading')
    rerender(<PluginSettingsForm pluginId="org.termul.example" />)
    await screen.findByLabelText('Greeting')
    const stale = snapshot()
    stale.values.greeting = 'Stale'
    await act(async () => old.resolve({ success: true, data: stale }))
    expect(screen.getByLabelText('Greeting')).toHaveValue('Hi')
  })

  it('ignores a stale save after changing pluginId', async () => {
    const old = deferred<IpcResult<ConfigSnapshot>>()
    vi.mocked(pluginApi.setConfig).mockReturnValueOnce(old.promise)
    const { rerender } = render(<PluginSettingsForm pluginId="org.termul.old" />)
    await screen.findByLabelText('Greeting')
    fireEvent.click(screen.getByRole('button', { name: 'settings.save' }))
    rerender(<PluginSettingsForm pluginId="org.termul.example" />)
    await screen.findByLabelText('Greeting')
    const stale = snapshot()
    stale.values.greeting = 'Stale'
    await act(async () => old.resolve({ success: true, data: stale }))
    expect(screen.getByLabelText('Greeting')).toHaveValue('Hi')
    expect(screen.getByRole('button', { name: 'settings.save' })).toBeEnabled()
  })
})
