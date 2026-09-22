import { legacyUpdateComponentPolicy } from '@shared/types/updater.types'
import { Store } from '@tauri-apps/plugin-store'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  clearPendingUpdatePlan,
  createPendingUpdatePlan,
  loadPendingUpdatePlan,
  savePendingUpdatePlan
} from '../tauri-update-plan-api'

const values = new Map<string, unknown>()
const store = {
  get: vi.fn(async <T>(key: string) => values.get(key) as T | undefined),
  set: vi.fn(async (key: string, value: unknown) => {
    values.set(key, value)
  }),
  delete: vi.fn(async (key: string) => {
    values.delete(key)
  }),
  save: vi.fn(async () => undefined)
}

describe('tauri-update-plan-api', () => {
  beforeEach(() => {
    values.clear()
    vi.clearAllMocks()
    vi.mocked(Store.load).mockResolvedValue(store as never)
  })

  it('persists and reloads a version-matched plan', async () => {
    const plan = createPendingUpdatePlan('2.4.0', legacyUpdateComponentPolicy('2.4.0'))

    await expect(savePendingUpdatePlan(plan)).resolves.toEqual({
      success: true,
      data: undefined
    })
    const loaded = await loadPendingUpdatePlan()
    expect(loaded.success).toBe(true)
    if (loaded.success) {
      expect(loaded.data).toMatchObject({
        ...plan,
        updatedAt: expect.any(String)
      })
    }
    expect(store.set).toHaveBeenCalledTimes(1)
    expect(store.save).toHaveBeenCalledTimes(1)
  })

  it('rejects a plan whose policy targets another version', async () => {
    values.set('pending_update_plan', {
      ...createPendingUpdatePlan('2.4.0', legacyUpdateComponentPolicy('2.3.0')),
      componentPolicy: legacyUpdateComponentPolicy('2.3.0')
    })

    const result = await loadPendingUpdatePlan()

    expect(result.success).toBe(false)
    if (!result.success) expect(result.code).toBe('UPDATE_PLAN_ERROR')
  })

  it('clears plans idempotently', async () => {
    const plan = createPendingUpdatePlan('2.4.0', legacyUpdateComponentPolicy('2.4.0'))
    await savePendingUpdatePlan(plan)

    await expect(clearPendingUpdatePlan()).resolves.toEqual({
      success: true,
      data: undefined
    })
    await expect(clearPendingUpdatePlan()).resolves.toEqual({
      success: true,
      data: undefined
    })
    await expect(loadPendingUpdatePlan()).resolves.toEqual({ success: true, data: null })
  })
})
