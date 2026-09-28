import { describe, expect, it } from 'vitest'
import type { SessionConfigOption, SessionModeState } from '@/lib/acp-api'
import {
  canonicalizeClaudeModelId,
  extractFastModeOption,
  filterDuplicateModeConfigOptions,
  flattenConfigOptionValues,
  isFastModeEnabled,
  isFastModeOption,
  normalizeSessionConfigOption,
  oppositeFastModeValue,
  partitionConfigOptions,
  resolveModelOption
} from './chat-input-bar-config'

function opt(id: string, category: string | null): SessionConfigOption {
  return {
    id,
    name: id,
    category,
    type: 'select',
    currentValue: 'a',
    description: null,
    options: [
      { value: 'a', name: 'A', description: null },
      { value: 'b', name: 'B', description: null }
    ]
  }
}

describe('partitionConfigOptions', () => {
  it('returns null thoughtLevel and empty rest for no options', () => {
    expect(partitionConfigOptions([])).toEqual({ model: null, thoughtLevel: null, rest: [] })
  })

  it('promotes a thought_level option and leaves rest empty', () => {
    const tl = opt('reasoning', 'thought_level')
    const result = partitionConfigOptions([tl])
    expect(result.model).toBeNull()
    expect(result.thoughtLevel).toBe(tl)
    expect(result.rest).toEqual([])
  })

  it('promotes a model option and keeps generic options in rest', () => {
    const mode = opt('mode', 'mode')
    const model = opt('model', 'model')
    const result = partitionConfigOptions([mode, model])
    expect(result.model).toBe(model)
    expect(result.thoughtLevel).toBeNull()
    expect(result.rest).toEqual([mode])
  })

  it('partitions mixed options, preserving rest order', () => {
    const mode = opt('mode', 'mode')
    const tl = opt('reasoning', 'thought_level')
    const model = opt('model', 'model')
    const result = partitionConfigOptions([mode, tl, model])
    expect(result.model).toBe(model)
    expect(result.thoughtLevel).toBe(tl)
    expect(result.rest).toEqual([mode])
  })

  it('treats unknown categories as generic rest', () => {
    const custom = opt('custom', 'something-new')
    const result = partitionConfigOptions([custom])
    expect(result.model).toBeNull()
    expect(result.thoughtLevel).toBeNull()
    expect(result.rest).toEqual([custom])
  })

  it('promotes only the first thought_level option, rest keeps the others', () => {
    const tl1 = opt('reasoning1', 'thought_level')
    const tl2 = opt('reasoning2', 'thought_level')
    const result = partitionConfigOptions([tl1, tl2])
    expect(result.model).toBeNull()
    expect(result.thoughtLevel).toBe(tl1)
    expect(result.rest).toEqual([tl2])
  })

  it('promotes only the first model option, rest keeps the others', () => {
    const model1 = opt('model1', 'model')
    const model2 = opt('model2', 'model')
    const result = partitionConfigOptions([model1, model2])
    expect(result.model).toBe(model1)
    expect(result.thoughtLevel).toBeNull()
    expect(result.rest).toEqual([model2])
  })

  it('flattens grouped model selectors so the picker gets leaf values', () => {
    const grouped = {
      id: 'model',
      name: 'Model',
      category: 'model',
      type: 'select',
      currentValue: 'claude-opus-4',
      options: [
        {
          group: 'claude',
          name: 'Claude',
          options: [
            { value: 'claude-opus-4', name: 'Opus 4' },
            { value: 'claude-sonnet-4', name: 'Sonnet 4' }
          ]
        },
        {
          group: 'bedrock',
          name: 'Bedrock',
          options: [{ value: 'bedrock-sonnet', name: 'Sonnet (Bedrock)' }]
        }
      ]
    } as unknown as SessionConfigOption
    const result = partitionConfigOptions([grouped])
    expect(result.model?.options).toEqual([
      { value: 'claude-opus-4', name: 'Opus 4', group: 'Claude' },
      { value: 'claude-sonnet-4', name: 'Sonnet 4', group: 'Claude' },
      { value: 'bedrock-sonnet', name: 'Sonnet (Bedrock)', group: 'Bedrock' }
    ])
    expect(result.model?.currentValue).toBe('claude-opus-4')
  })
})

describe('filterDuplicateModeConfigOptions', () => {
  const modes: SessionModeState = {
    currentModeId: 'agent',
    availableModes: [
      { id: 'agent', name: 'Agent' },
      { id: 'plan', name: 'Plan' }
    ]
  }

  it('keeps mode config options when native modes are absent', () => {
    const mode = opt('mode', 'mode')
    expect(filterDuplicateModeConfigOptions([mode], null)).toEqual([mode])
  })

  it('removes mode config options when native modes are present', () => {
    const mode = opt('mode', 'mode')
    const custom = opt('custom', 'custom')
    expect(filterDuplicateModeConfigOptions([mode, custom], modes)).toEqual([custom])
  })
})

describe('fast mode helpers', () => {
  function fastMode(currentValue = 'off'): SessionConfigOption {
    return {
      id: 'fast_mode',
      name: 'Fast Mode',
      category: 'other',
      type: 'select',
      currentValue,
      description: null,
      options: [
        { value: 'on', name: 'On', description: null },
        { value: 'off', name: 'Off', description: null }
      ]
    }
  }

  it('detects binary Fast Mode options and ignores unrelated selects', () => {
    expect(isFastModeOption(fastMode())).toBe(true)
    expect(isFastModeOption(opt('custom', 'other'))).toBe(false)
    expect(
      isFastModeOption({
        ...fastMode(),
        id: 'speed',
        name: 'Speed'
      })
    ).toBe(false)
  })

  it('resolves enabled state and opposite value', () => {
    expect(isFastModeEnabled(fastMode('off'))).toBe(false)
    expect(isFastModeEnabled(fastMode('on'))).toBe(true)
    expect(oppositeFastModeValue(fastMode('off'))).toBe('on')
    expect(oppositeFastModeValue(fastMode('on'))).toBe('off')
  })

  it('extracts Fast Mode from a generic options list', () => {
    const custom = opt('custom', 'other')
    const fm = fastMode('off')
    expect(extractFastModeOption([custom, fm])).toEqual({
      fastMode: fm,
      rest: [custom]
    })
    expect(extractFastModeOption([custom])).toEqual({ fastMode: null, rest: [custom] })
  })
})

describe('grouped config option flattening', () => {
  it('returns an empty list for missing or non-array options', () => {
    expect(flattenConfigOptionValues(undefined)).toEqual([])
    expect(flattenConfigOptionValues(null)).toEqual([])
  })

  it('preserves already-flat options by identity', () => {
    const option = opt('model', 'model')
    expect(normalizeSessionConfigOption(option)).toBe(option)
  })

  it('flattens parent-with-value groups to versioned Claude leaves', () => {
    expect(
      flattenConfigOptionValues([
        {
          name: 'Sonnet',
          value: 'claude-sonnet',
          options: [
            { value: 'claude-sonnet-5', name: 'Sonnet 5' },
            { value: '[1m]', name: 'Opus (1M context)' }
          ]
        }
      ] as unknown as SessionConfigOption['options'])
    ).toEqual([
      { value: 'claude-sonnet-5', name: 'Sonnet 5', group: 'Sonnet' },
      { value: 'claude-sonnet-5[1m]', name: 'Opus (1M context)', group: 'Sonnet' }
    ])
  })

  it('rewrites family-only Claude ids so ACP receives the versioned value', () => {
    expect(canonicalizeClaudeModelId('claude-sonnet[1m]')).toBe('claude-sonnet-5[1m]')
    expect(canonicalizeClaudeModelId('claude-opus')).toBe('claude-opus-5')
    expect(canonicalizeClaudeModelId('claude-sonnet-5[1m]')).toBe('claude-sonnet-5[1m]')
    expect(canonicalizeClaudeModelId('claude-sonnet-4-5[1m]')).toBe('claude-sonnet-4-5[1m]')
  })

  it('falls through to native session.models when the config option has no leaf values', () => {
    const emptyModel = {
      id: 'model',
      name: 'Model',
      category: 'model',
      type: 'select',
      currentValue: 'm1',
      options: []
    }
    const resolved = resolveModelOption(emptyModel, {
      currentModelId: 'openrouter/gpt-5.5',
      availableModels: [{ modelId: 'openrouter/gpt-5.5', name: 'GPT-5.5' }]
    })
    expect(resolved.source).toBe('models')
    expect(resolved.option?.currentValue).toBe('openrouter/gpt-5.5')
    expect(resolved.option?.options).toEqual([
      { value: 'openrouter/gpt-5.5', name: 'GPT-5.5', description: undefined }
    ])
  })
})
