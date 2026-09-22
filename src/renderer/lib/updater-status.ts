import type {
  PendingUpdatePlan,
  UpdateComponent,
  UpdateComponentAction,
  UpdateComponentPolicy
} from '@shared/types/updater.types'

export interface UpdateComponentImpact {
  component: UpdateComponent
  action: UpdateComponentAction
}

export function getUpdateComponentImpact(
  policy: UpdateComponentPolicy | null | undefined
): UpdateComponentImpact[] {
  if (!policy) return []

  return (
    Object.entries(policy.components) as [UpdateComponent, { action: UpdateComponentAction }][]
  )
    .filter(([, entry]) => entry.action !== 'preserve')
    .map(([component, entry]) => ({ component, action: entry.action }))
}

export function getPendingUpdateStatus(
  plan: PendingUpdatePlan | null | undefined
): 'deferred' | 'failed' | 'reconciling' | 'completed' | null {
  if (!plan || plan.status === 'prepared') return null
  return plan.status
}
