import { describe, expect, it } from 'vitest'
import {
  conversationSnapshotToPending,
  hasComposerSnapshot,
  hydrateComposerControls,
  sessionHasComposerControls,
  snapshotSessionComposer
} from './conversation-composer'

describe('conversation-composer', () => {
  it('snapshots live session controls including display names', () => {
    const snapshot = snapshotSessionComposer(
      {
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        models: {
          currentModelId: 'opus',
          availableModels: [
            { modelId: 'opus', name: 'Opus' },
            { modelId: 'sonnet', name: 'Sonnet' }
          ]
        },
        modes: {
          currentModeId: 'plan',
          availableModes: [
            { id: 'agent', name: 'Agent' },
            { id: 'plan', name: 'Plan' }
          ]
        },
        configOptions: [
          {
            id: 'thought_level',
            name: 'Thinking',
            type: 'select',
            currentValue: 'high',
            options: [
              { value: 'low', name: 'Low' },
              { value: 'high', name: 'High' }
            ]
          }
        ]
      },
      'cfg-pi'
    )
    expect(snapshot).toEqual({
      agentConfigId: 'cfg-pi',
      modelId: 'opus',
      modelName: 'Opus',
      modeId: 'plan',
      modeName: 'Plan',
      configValues: { thought_level: 'high' },
      configLabels: { thought_level: { optionName: 'Thinking', valueName: 'High' } }
    })
    expect(hasComposerSnapshot(snapshot)).toBe(true)
    expect(conversationSnapshotToPending(snapshot)).toEqual({
      modelId: 'opus',
      modeId: 'plan',
      configValues: { thought_level: 'high' }
    })
  })

  it('hydrates empty closed-session controls from a snapshot', () => {
    const controls = hydrateComposerControls({
      agentConfigId: 'cfg-pi',
      modelId: 'opus',
      modelName: 'Opus',
      modeId: 'plan',
      modeName: 'Plan',
      configValues: { thought_level: 'high' },
      configLabels: { thought_level: { optionName: 'Thinking', valueName: 'High' } }
    })
    expect(controls.models).toEqual({
      currentModelId: 'opus',
      availableModels: [{ modelId: 'opus', name: 'Opus' }]
    })
    expect(controls.modes).toEqual({
      currentModeId: 'plan',
      availableModes: [{ id: 'plan', name: 'Plan' }]
    })
    expect(controls.configOptions[0]).toMatchObject({
      id: 'thought_level',
      currentValue: 'high'
    })
    expect(sessionHasComposerControls(controls)).toBe(true)
  })

  it('overlays snapshot currents onto a cached option list', () => {
    const controls = hydrateComposerControls(
      { modelId: 'sonnet', modeId: 'ask' },
      {
        models: {
          currentModelId: 'opus',
          availableModels: [
            { modelId: 'opus', name: 'Opus' },
            { modelId: 'sonnet', name: 'Sonnet' }
          ]
        },
        modes: {
          currentModeId: 'agent',
          availableModes: [
            { id: 'agent', name: 'Agent' },
            { id: 'ask', name: 'Ask' }
          ]
        }
      }
    )
    expect(controls.models?.currentModelId).toBe('sonnet')
    expect(controls.models?.availableModels).toHaveLength(2)
    expect(controls.modes?.currentModeId).toBe('ask')
  })
})
