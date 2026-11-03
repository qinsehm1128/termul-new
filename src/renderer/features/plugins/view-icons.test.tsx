import viewIcons from '@shared/plugin-fixtures/view-icons.json'
import { render } from '@testing-library/react'
import { Puzzle } from 'lucide-react'
import { describe, expect, it } from 'vitest'
import { getPluginViewIcon, pluginViewIcons } from './view-icons'

describe('host-owned plugin view icons', () => {
  it('maps every allowlisted icon to a renderable Lucide component, with no extra names', () => {
    expect(Object.keys(pluginViewIcons).sort()).toEqual([...viewIcons].sort())
    for (const name of viewIcons) {
      expect(pluginViewIcons[name]).toBeDefined()
      const Icon = getPluginViewIcon(name)
      const { container, unmount } = render(<Icon aria-hidden="true" />)
      expect(container.querySelector('svg')).not.toBeNull()
      unmount()
    }
  })

  it('defaults missing or invalid names to the host puzzle icon', () => {
    expect(getPluginViewIcon()).toBe(Puzzle)
    expect(getPluginViewIcon('unknown')).toBe(Puzzle)
    expect(getPluginViewIcon('__proto__')).toBe(Puzzle)
  })
})
