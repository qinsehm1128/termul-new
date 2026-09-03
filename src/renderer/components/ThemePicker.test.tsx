/**
 * T-G1A — a user-imported theme has to reach the picker list.
 *
 * `COLOR_THEME_FAMILIES` / `THEME_PICKER_ROWS` used to be module-level `const`s
 * evaluated at import time, so a theme loaded after that first tick could never
 * appear no matter how well the resolver knew it. This asserts the list is
 * rebuilt from the registry, through the real load path.
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { setCustomColorThemes } from '@/lib/themes/custom-theme-registry'
import { useThemePickerStore } from '@/stores/theme-picker-store'
import { ThemePicker } from './ThemePicker'

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  write: vi.fn(),
  updateSettings: vi.fn()
}))

vi.mock('@/lib/persistence-api', () => ({
  CUSTOM_THEMES_KEY: 'themes/custom',
  persistenceApi: { read: mocks.read, write: mocks.write }
}))

vi.mock('@/hooks/use-app-settings', () => ({
  useUpdateAppSettings: () => mocks.updateSettings
}))

vi.mock('@/hooks/use-color-theme', () => ({
  useEffectiveColorThemeId: () => 'dracula'
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key })
}))

const SUNSET = {
  id: 'sunset',
  name: 'Sunset',
  appearance: 'dark',
  familyId: 'sunset',
  dark: {
    palette: {
      neutral: '#101010',
      ink: '#f0f0f0',
      primary: '#ff8844',
      accent: '#ff4488',
      success: '#44ff88',
      warning: '#ffcc44',
      error: '#ff4444',
      info: '#4488ff'
    }
  }
}

beforeEach(() => {
  // jsdom has no layout, so it ships no `scrollIntoView`; the picker calls it
  // on every focus move.
  Element.prototype.scrollIntoView = vi.fn()
  setCustomColorThemes([])
  mocks.read.mockReset()
  mocks.write.mockReset()
  mocks.updateSettings.mockReset()
  mocks.updateSettings.mockResolvedValue(undefined)
  mocks.read.mockResolvedValue({ success: true, data: { themes: [SUNSET] } })
  useThemePickerStore.getState().open('dracula')
})

describe('ThemePicker custom themes', () => {
  it('lists a persisted custom theme and applies it on click', async () => {
    render(<ThemePicker />)

    const row = await screen.findByRole('option', { name: /Sunset/ })
    fireEvent.click(row)

    await waitFor(() => {
      expect(mocks.updateSettings).toHaveBeenCalledWith({
        colorTheme: 'sunset',
        appearanceMode: 'dark'
      })
    })
  })

  it('still lists the bundled themes alongside it', async () => {
    render(<ThemePicker />)

    await screen.findByRole('option', { name: /Sunset/ })
    expect(screen.getByRole('option', { name: /^Dracula/ })).toBeInTheDocument()
  })
})
