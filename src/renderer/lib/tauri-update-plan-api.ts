import type { IpcResult } from '@shared/types/ipc.types'
import {
  type PendingUpdatePlan,
  parseUpdateComponentPolicy,
  UPDATE_COMPONENTS,
  type UpdateComponent,
  type UpdateComponentPolicy
} from '@shared/types/updater.types'
import { Store } from '@tauri-apps/plugin-store'

const PLAN_STORE_FILE = 'update-plan.json'
const PENDING_PLAN_KEY = 'pending_update_plan'

let planStore: Store | null = null

async function getPlanStore(): Promise<Store> {
  if (!planStore) {
    planStore = await Store.load(PLAN_STORE_FILE, {
      autoSave: false,
      defaults: {}
    })
  }
  return planStore
}

function errorResult<T>(error: unknown, fallback: string): IpcResult<T> {
  return {
    success: false,
    error: error instanceof Error && error.message.trim() ? error.message : fallback,
    code: 'UPDATE_PLAN_ERROR'
  }
}

export function createPendingUpdatePlan(
  targetVersion: string,
  componentPolicy: UpdateComponentPolicy
): PendingUpdatePlan {
  const now = new Date().toISOString()
  return {
    schemaVersion: 1,
    targetVersion,
    componentPolicy,
    status: 'prepared',
    currentComponentBuildIds: {},
    requiredActions: UPDATE_COMPONENTS.filter(
      (component) => componentPolicy.components[component].action !== 'preserve'
    ),
    deferredComponents: [],
    createdAt: now,
    updatedAt: now
  }
}

export async function loadPendingUpdatePlan(): Promise<IpcResult<PendingUpdatePlan | null>> {
  try {
    const store = await getPlanStore()
    const plan = await store.get<PendingUpdatePlan>(PENDING_PLAN_KEY)
    if (!plan) return { success: true, data: null }
    if (plan.schemaVersion !== 1 || typeof plan.targetVersion !== 'string') {
      return {
        success: false,
        error: 'Pending update plan is invalid',
        code: 'UPDATE_PLAN_INVALID'
      }
    }
    let policy: PendingUpdatePlan['componentPolicy']
    try {
      policy = parseUpdateComponentPolicy(plan.componentPolicy, plan.targetVersion)
    } catch (error) {
      return errorResult(error, 'Pending update plan component policy is invalid')
    }
    if (
      typeof plan.currentComponentBuildIds !== 'object' ||
      plan.currentComponentBuildIds === null ||
      Array.isArray(plan.currentComponentBuildIds) ||
      !Array.isArray(plan.requiredActions)
    ) {
      return {
        success: false,
        error: 'Pending update plan action metadata is invalid',
        code: 'UPDATE_PLAN_INVALID'
      }
    }
    if (!Array.isArray(plan.deferredComponents)) {
      return {
        success: false,
        error: 'Pending update plan deferred components are invalid',
        code: 'UPDATE_PLAN_INVALID'
      }
    }
    const validStatuses = new Set<PendingUpdatePlan['status']>([
      'prepared',
      'reconciling',
      'deferred',
      'completed',
      'failed'
    ])
    if (!validStatuses.has(plan.status)) {
      return {
        success: false,
        error: 'Pending update plan status is invalid',
        code: 'UPDATE_PLAN_INVALID'
      }
    }
    const isComponent = (value: unknown): value is UpdateComponent =>
      UPDATE_COMPONENTS.includes(value as UpdateComponent)
    if (
      plan.requiredActions.some((component) => !isComponent(component)) ||
      plan.deferredComponents.some((component) => !isComponent(component)) ||
      Object.values(plan.currentComponentBuildIds).some(
        (buildId) => typeof buildId !== 'string' || buildId.trim() === ''
      )
    ) {
      return {
        success: false,
        error: 'Pending update plan action metadata is invalid',
        code: 'UPDATE_PLAN_INVALID'
      }
    }
    return {
      success: true,
      data: {
        ...plan,
        componentPolicy: policy,
        currentComponentBuildIds: { ...plan.currentComponentBuildIds },
        requiredActions: [...plan.requiredActions],
        deferredComponents: [...plan.deferredComponents]
      }
    }
  } catch (error) {
    return errorResult(error, 'Failed to load pending update plan')
  }
}

export async function savePendingUpdatePlan(plan: PendingUpdatePlan): Promise<IpcResult<void>> {
  try {
    const store = await getPlanStore()
    const policy = parseUpdateComponentPolicy(plan.componentPolicy, plan.targetVersion)
    if (
      plan.schemaVersion !== 1 ||
      !Array.isArray(plan.requiredActions) ||
      !Array.isArray(plan.deferredComponents) ||
      typeof plan.currentComponentBuildIds !== 'object' ||
      plan.currentComponentBuildIds === null ||
      Array.isArray(plan.currentComponentBuildIds)
    ) {
      return {
        success: false,
        error: 'Pending update plan action metadata is invalid',
        code: 'UPDATE_PLAN_INVALID'
      }
    }
    await store.set(PENDING_PLAN_KEY, {
      ...plan,
      componentPolicy: policy,
      updatedAt: new Date().toISOString()
    })
    await store.save()
    return { success: true, data: undefined }
  } catch (error) {
    return errorResult(error, 'Failed to save pending update plan')
  }
}

export async function clearPendingUpdatePlan(): Promise<IpcResult<void>> {
  try {
    const store = await getPlanStore()
    await store.delete(PENDING_PLAN_KEY)
    await store.save()
    return { success: true, data: undefined }
  } catch (error) {
    return errorResult(error, 'Failed to clear pending update plan')
  }
}
