import { describe, expect, it } from 'vitest'
import {
  emptyPendingLauncherOptions,
  hasPendingLauncherOptions,
  overlayPendingLauncherOptions
} from './pending-launcher-options'

describe('pending-launcher-options', () => {
  it('overlays pending model/mode/config onto cached options', () => {
    const overlaid = overlayPendingLauncherOptions({
      models: {
        currentModelId: 'm1',
        availableModels: [
          { modelId: 'm1', name: 'One' },
          { modelId: 'm2', name: 'Two' }
        ]
      },
      modes: {
        currentModeId: 'agent',
        availableModes: [
          { id: 'agent', name: 'Agent' },
          { id: 'plan', name: 'Plan' }
        ]
      },
      configOptions: [
        {
          id: 'thought_level',
          name: 'Thinking',
          category: 'thought_level',
          type: 'select',
          currentValue: 'low',
          options: [
            { value: 'low', name: 'Low' },
            { value: 'high', name: 'High' }
          ]
        }
      ],
      pending: {
        modelId: 'm2',
        modeId: 'plan',
        configValues: { thought_level: 'high' }
      }
    })

    expect(overlaid.models?.currentModelId).toBe('m2')
    expect(overlaid.modes?.currentModeId).toBe('plan')
    expect(overlaid.configOptions[0]?.currentValue).toBe('high')
    expect(hasPendingLauncherOptions(emptyPendingLauncherOptions())).toBe(false)
    expect(hasPendingLauncherOptions({ modelId: 'm2', configValues: {} })).toBe(true)
  })

  it('canonicalizes family-only Claude ids on pending model overlays', () => {
    const overlaid = overlayPendingLauncherOptions({
      models: {
        currentModelId: 'claude-sonnet',
        availableModels: [{ modelId: 'claude-sonnet-5[1m]', name: 'Opus (1M context)' }]
      },
      modes: null,
      configOptions: [
        {
          id: 'model',
          name: 'Model',
          category: 'model',
          type: 'select',
          currentValue: 'claude-sonnet',
          options: [{ value: 'claude-sonnet-5[1m]', name: 'Opus (1M context)' }]
        }
      ],
      pending: {
        modelId: 'claude-sonnet[1m]',
        configValues: { model: 'claude-sonnet[1m]' }
      }
    })
    expect(overlaid.models?.currentModelId).toBe('claude-sonnet-5[1m]')
    expect(overlaid.configOptions[0]?.currentValue).toBe('claude-sonnet-5[1m]')
  })
})
