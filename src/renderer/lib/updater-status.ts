import {
  type PendingUpdatePlan,
  type PendingUpdatePlanStatus,
  UPDATE_COMPONENTS,
  type UpdateComponent,
  type UpdateComponentAction,
  type UpdateComponentPolicy
} from '@shared/types/updater.types'
import { runtimeT } from '@/i18n/runtime'

/**
 * User-facing update semantics.
 *
 * Policy actions already say whether a component matches (`preserve`), must be
 * replaced (`restart`), or waits on an active terminal (`defer-if-active`).
 * Durable plan status says what reconciliation already decided. This module
 * must not read a live Core PID or compare build IDs.
 */

export type UpdateCopyTranslate = (key: string, values?: Record<string, string>) => string

export type UpdateCoreComponent = 'acpCore' | 'terminalCore'

export type VisiblePendingUpdateStatus = Exclude<PendingUpdatePlanStatus, 'prepared'>

export interface UpdateComponentImpact {
  component: UpdateComponent
  action: Exclude<UpdateComponentAction, 'preserve'>
}

export interface UpdateInstallSemantics {
  /** Installing an update always relaunches the GUI shell. */
  guiRestarts: true
  /** The GUI relaunch itself does not kill PTYs owned by Terminal Core. */
  guiRelaunchKeepsCorePty: true
  /** A Core replacement is a reconnect, never a zero-interruption handoff. */
  reconnectNotZeroInterruption: true
  /** No policy was supplied, so Core identity must not be inferred. */
  metadataUnavailable: boolean
  /** Cores declared `preserve`: matching build, adopt and reconnect. */
  matchingCores: UpdateCoreComponent[]
  /** Cores declared `restart`: mismatched build, replace. */
  mismatchedRestartCores: UpdateCoreComponent[]
  /** `terminalCore` is `defer-if-active` and a PTY is still active. */
  terminalDeferred: boolean
  /** `terminalCore` is `defer-if-active` and no PTY is active. */
  terminalReplaceNow: boolean
  /** `terminalCore` is an immediate `restart` while a PTY is still active. */
  terminalRestartDespiteActive: boolean
  /** Non-terminal `defer-if-active` entries. These are not active-PTY defers. */
  otherDeferred: UpdateComponent[]
  unsupportedComponents: UpdateComponent[]
}

export interface UpdateInstallMessagePart {
  key: string
  components?: readonly UpdateComponent[]
}

export interface PendingUpdatePresentation {
  status: VisiblePendingUpdateStatus
  tone: 'progress' | 'warning' | 'danger' | 'success'
  summary: string
  lastError: string | null
  offerRetry: boolean
  /** Summary plus durable lastError, for compact surfaces such as the status bar. */
  label: string
}

const CORE_COMPONENTS: readonly UpdateCoreComponent[] = ['acpCore', 'terminalCore']

const PENDING_TONE: Record<VisiblePendingUpdateStatus, PendingUpdatePresentation['tone']> = {
  reconciling: 'progress',
  deferred: 'warning',
  failed: 'danger',
  completed: 'success'
}

function isCoreComponent(component: UpdateComponent): component is UpdateCoreComponent {
  return component === 'acpCore' || component === 'terminalCore'
}

function isKnownComponent(component: UpdateComponent): boolean {
  return UPDATE_COMPONENTS.includes(component)
}

export function translateUpdateCopy(key: string, values?: Record<string, string>): string {
  return runtimeT('shell', key, key, values)
}

export function getUpdateComponentImpact(
  policy: UpdateComponentPolicy | null | undefined
): UpdateComponentImpact[] {
  if (!policy) return []

  const impact: UpdateComponentImpact[] = []
  for (const component of UPDATE_COMPONENTS) {
    const action = policy.components[component]?.action
    if (!action || action === 'preserve') continue
    impact.push({ component, action })
  }
  return impact
}

export function getMatchingCoreComponents(
  policy: UpdateComponentPolicy | null | undefined
): UpdateCoreComponent[] {
  if (!policy) return []
  return CORE_COMPONENTS.filter((component) => policy.components[component]?.action === 'preserve')
}

export function getUpdateComponentImpactMessageKey(impact: UpdateComponentImpact): string {
  if (impact.action === 'unsupported') return 'updates.componentImpact.unsupported'
  if (impact.action === 'defer-if-active') {
    return impact.component === 'terminalCore'
      ? 'updates.componentImpact.deferIfActive'
      : 'updates.componentImpact.deferGeneric'
  }
  if (impact.component === 'renderer' || impact.component === 'guiNative') {
    return 'updates.componentImpact.guiRestart'
  }
  return 'updates.componentImpact.coreRestart'
}

export function describeUpdateInstallSemantics(input: {
  policy: UpdateComponentPolicy | null | undefined
  hasActiveTerminalSessions: boolean
}): UpdateInstallSemantics {
  const semantics: UpdateInstallSemantics = {
    guiRestarts: true,
    guiRelaunchKeepsCorePty: true,
    reconnectNotZeroInterruption: true,
    metadataUnavailable: !input.policy,
    matchingCores: [],
    mismatchedRestartCores: [],
    terminalDeferred: false,
    terminalReplaceNow: false,
    terminalRestartDespiteActive: false,
    otherDeferred: [],
    unsupportedComponents: []
  }
  if (!input.policy) return semantics

  for (const component of UPDATE_COMPONENTS) {
    const action = input.policy.components[component]?.action
    if (!action || action === 'preserve') {
      if (action === 'preserve' && isCoreComponent(component)) {
        semantics.matchingCores.push(component)
      }
      continue
    }
    if (action === 'unsupported') {
      semantics.unsupportedComponents.push(component)
      continue
    }
    if (component === 'terminalCore' && action === 'defer-if-active') {
      if (input.hasActiveTerminalSessions) semantics.terminalDeferred = true
      else semantics.terminalReplaceNow = true
      continue
    }
    if (action === 'defer-if-active') {
      semantics.otherDeferred.push(component)
      continue
    }
    if (isCoreComponent(component) && action === 'restart') {
      semantics.mismatchedRestartCores.push(component)
      if (component === 'terminalCore' && input.hasActiveTerminalSessions) {
        semantics.terminalRestartDespiteActive = true
      }
    }
  }

  return semantics
}

export function getUpdateInstallMessageParts(
  semantics: UpdateInstallSemantics
): UpdateInstallMessagePart[] {
  const parts: UpdateInstallMessagePart[] = [
    { key: 'updates.installSemantics.guiRestart' },
    { key: 'updates.installSemantics.ptySurvivesGui' }
  ]

  if (semantics.metadataUnavailable) {
    parts.push({ key: 'updates.installSemantics.unavailable' })
  } else {
    if (semantics.matchingCores.length > 0) {
      parts.push({
        key: 'updates.installSemantics.coreAdopt',
        components: semantics.matchingCores
      })
    }
    if (semantics.mismatchedRestartCores.length > 0) {
      parts.push({
        key: 'updates.installSemantics.coreRestart',
        components: semantics.mismatchedRestartCores
      })
    }
    if (semantics.terminalDeferred) parts.push({ key: 'updates.installSemantics.terminalDefer' })
    if (semantics.terminalReplaceNow) {
      parts.push({ key: 'updates.installSemantics.terminalReplaceIdle' })
    }
    if (semantics.terminalRestartDespiteActive) {
      parts.push({ key: 'updates.installSemantics.terminalRestartActive' })
    }
    if (semantics.otherDeferred.length > 0) {
      parts.push({
        key: 'updates.installSemantics.deferGeneric',
        components: semantics.otherDeferred
      })
    }
    if (semantics.unsupportedComponents.length > 0) {
      parts.push({
        key: 'updates.installSemantics.unsupported',
        components: semantics.unsupportedComponents
      })
    }
  }

  parts.push({ key: 'updates.installSemantics.reconnect' })
  return parts
}

function componentList(
  components: readonly UpdateComponent[],
  translate: UpdateCopyTranslate
): string {
  return components.map((component) => translate(`updates.componentImpact.${component}`)).join(', ')
}

export function getUpdateImpactLines(
  policy: UpdateComponentPolicy | null | undefined,
  translate: UpdateCopyTranslate
): string[] {
  if (!policy) return [translate('updates.componentImpact.unavailable')]

  const lines = [translate('updates.componentImpact.summary')]
  const matching = getMatchingCoreComponents(policy)
  if (matching.length > 0) {
    lines.push(
      translate('updates.installSemantics.coreAdopt', {
        components: componentList(matching, translate)
      })
    )
  }
  for (const impact of getUpdateComponentImpact(policy)) {
    lines.push(
      `${translate(`updates.componentImpact.${impact.component}`)} ${translate(
        getUpdateComponentImpactMessageKey(impact)
      )}`
    )
  }
  return lines
}

export function buildUpdateInstallConfirmation(input: {
  policy: UpdateComponentPolicy | null | undefined
  hasActiveTerminalSessions: boolean
  version: string
  translate: UpdateCopyTranslate
}): string {
  const semantics = describeUpdateInstallSemantics(input)
  const details = getUpdateInstallMessageParts(semantics)
    .map((part) =>
      input.translate(
        part.key,
        part.components
          ? { version: input.version, components: componentList(part.components, input.translate) }
          : { version: input.version }
      )
    )
    .join(' ')
  return input.translate('updates.installSemantics.lead', {
    version: input.version,
    details
  })
}

export function getPendingUpdateStatus(
  plan: PendingUpdatePlan | null | undefined
): VisiblePendingUpdateStatus | null {
  if (!plan || plan.status === 'prepared') return null
  return plan.status
}

export function presentPendingUpdate(
  plan: PendingUpdatePlan | null | undefined,
  translate: UpdateCopyTranslate
): PendingUpdatePresentation | null {
  const status = getPendingUpdateStatus(plan)
  if (!plan || !status) return null

  const deferredComponents =
    status === 'deferred'
      ? plan.deferredComponents.filter((component) => isKnownComponent(component))
      : []
  const summary =
    status === 'deferred' && deferredComponents.length > 0
      ? translate('updates.reconciliation.deferredComponents', {
          components: componentList(deferredComponents, translate)
        })
      : translate(`updates.reconciliation.${status}`)
  const lastError = status === 'failed' && plan.lastError?.trim() ? plan.lastError.trim() : null

  return {
    status,
    tone: PENDING_TONE[status],
    summary,
    lastError,
    offerRetry: status === 'failed' || status === 'deferred',
    label: lastError ? `${summary} ${lastError}` : summary
  }
}
