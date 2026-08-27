import type { ConversationId } from '@shared/types/conversation.types'
import type {
  RecoveryAction,
  RecoveryActionResult,
  RecoveryItemV1
} from '@shared/types/conversation-recovery.types'
import { AlertTriangle, CheckCircle2, LoaderCircle, ShieldCheck } from 'lucide-react'
import { useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { loadSessionWorkspace } from '@/hooks/use-session-workspace-sync'
import { conversationApi } from '@/lib/conversation-api'
import { logFrontendError } from '@/lib/log-api'
import { randomUUID } from '@/lib/uuid'
import { useConversationStore } from '@/stores/conversation-store'

export interface ConversationRecoveryPanelProps {
  items?: readonly RecoveryItemV1[]
  conversationId?: ConversationId | null
  className?: string
  embedded?: boolean
  onItemsChange?: (items: readonly RecoveryItemV1[]) => void
}

function actionStateKey(recoveryId: string, revision: number): string {
  return `${recoveryId}:${revision}`
}

function requestForAction(
  item: RecoveryItemV1,
  conversationId: ConversationId,
  action: RecoveryAction['action'],
  idempotencyKey?: string
): RecoveryAction {
  const common = { recoveryId: item.recoveryId, expectedRevision: item.revision }
  switch (action) {
    case 'inspect':
      return { ...common, action, payload: {} }
    case 'associateConversation':
      return {
        ...common,
        action,
        idempotencyKey: idempotencyKey ?? randomUUID(),
        payload: { conversationId }
      }
    case 'startEmptyWorkspace':
      return {
        ...common,
        action,
        idempotencyKey: idempotencyKey ?? randomUUID(),
        payload: { conversationId, expectedWorkspaceRevision: null }
      }
    case 'dismissPreservedSource':
      return {
        ...common,
        action,
        idempotencyKey: idempotencyKey ?? randomUUID(),
        payload: { reasonCode: 'deferLegacyProjection' }
      }
  }
}

function actionConversationId(
  item: RecoveryItemV1,
  preferred: ConversationId | null | undefined
): ConversationId | null {
  return preferred ?? item.conversationIds[0] ?? null
}

function responseMatchesRequest(request: RecoveryAction, result: RecoveryActionResult): boolean {
  const expectedRevision = request.expectedRevision + (request.action === 'inspect' ? 0 : 1)
  const expectedAuthorization = request.action === 'inspect' ? 'read' : 'mutation'
  return (
    result.recoveryId === request.recoveryId &&
    result.action === request.action &&
    result.authorization === expectedAuthorization &&
    result.recoveryRevision === expectedRevision
  )
}

function snapshotInspectedEvidence(result: RecoveryActionResult): RecoveryActionResult {
  return {
    ...result,
    sourcePaths: [...result.sourcePaths],
    sourceSha256: [...result.sourceSha256],
    candidateFacts: result.candidateFacts.map((fact) => ({ ...fact })),
    provenance: result.provenance.map((entry) => ({ ...entry }))
  }
}

export function ConversationRecoveryPanel({
  items,
  conversationId,
  className,
  embedded = false,
  onItemsChange
}: ConversationRecoveryPanelProps): React.JSX.Element | null {
  const { t } = useTranslation('conversation')
  const storeItems = useConversationStore((state) => state.recoveryItems)
  const setRecoveryItems = useConversationStore((state) => state.setRecoveryItems)
  const sourceItems = items ?? storeItems
  const [results, setResults] = useState<Record<string, RecoveryActionResult | undefined>>({})
  const [inspectedEvidence, setInspectedEvidence] = useState<
    Record<string, RecoveryActionResult | undefined>
  >({})
  const [errors, setErrors] = useState<Record<string, string | undefined>>({})
  const [running, setRunning] = useState<Record<string, RecoveryAction['action'] | undefined>>({})
  const idempotencyKeys = useRef<Record<string, string>>({})
  const liveItemsRef = useRef(sourceItems)
  liveItemsRef.current = sourceItems
  const visibleItems = useMemo(() => [...sourceItems], [sourceItems])

  if (visibleItems.length === 0) return null

  const run = async (item: RecoveryItemV1, action: RecoveryAction['action']): Promise<void> => {
    const canonicalId = actionConversationId(item, conversationId)
    if (!canonicalId && action !== 'inspect' && action !== 'dismissPreservedSource') {
      setErrors((current) => ({
        ...current,
        [actionStateKey(item.recoveryId, item.revision)]: 'CONVERSATION_INVALID_ID'
      }))
      return
    }
    const idempotencyKeySlot = `${item.recoveryId}:${item.revision}:${action}`
    let idempotencyKey: string | undefined
    if (action !== 'inspect') {
      idempotencyKey = idempotencyKeys.current[idempotencyKeySlot]
      if (!idempotencyKey) {
        idempotencyKey = randomUUID()
        idempotencyKeys.current[idempotencyKeySlot] = idempotencyKey
      }
    }
    const request = requestForAction(
      item,
      canonicalId ?? ('' as ConversationId),
      action,
      idempotencyKey
    )
    const requestKey = actionStateKey(item.recoveryId, item.revision)
    const requestedRevision = item.revision
    setRunning((current) => ({ ...current, [requestKey]: action }))
    setErrors((current) => ({ ...current, [requestKey]: undefined }))
    if (action === 'inspect') {
      setInspectedEvidence((current) => ({ ...current, [requestKey]: undefined }))
    }
    try {
      const result = await conversationApi.resolveRecovery(request)
      const live = liveItemsRef.current.find(
        (candidate) => candidate.recoveryId === item.recoveryId
      )
      if (!live || live.revision !== requestedRevision) {
        return
      }
      if (!result.success) {
        setErrors((current) => ({ ...current, [requestKey]: result.code }))
        void logFrontendError({
          level: 'warn',
          source: 'conversation-recovery-panel',
          message: `recoveryId=${item.recoveryId} revision=${item.revision} action=${action} code=${result.code}`
        })
        return
      }
      if (!responseMatchesRequest(request, result.data)) {
        setErrors((current) => ({
          ...current,
          [requestKey]: 'CONVERSATION_RECOVERY_FAILED'
        }))
        void logFrontendError({
          level: 'warn',
          source: 'conversation-recovery-panel',
          message: `recoveryId=${item.recoveryId} revision=${item.revision} action=${action} code=CONVERSATION_RECOVERY_FAILED outcome=mismatched_response`
        })
        return
      }
      setResults((current) => ({ ...current, [requestKey]: result.data }))
      if (action === 'inspect') {
        setInspectedEvidence((current) => ({
          ...current,
          [requestKey]: snapshotInspectedEvidence(result.data)
        }))
      } else {
        const updated = sourceItems.map((candidate) =>
          candidate.recoveryId === item.recoveryId
            ? {
                ...candidate,
                status: result.data.status,
                revision: result.data.recoveryRevision
              }
            : candidate
        )
        if (!items) setRecoveryItems(updated)
        onItemsChange?.(updated)
      }
      if (result.data.workspaceChanged && canonicalId) {
        await loadSessionWorkspace(canonicalId)
      }
    } catch {
      if (action === 'inspect') {
        setInspectedEvidence((current) => ({ ...current, [requestKey]: undefined }))
      }
      setErrors((current) => ({
        ...current,
        [requestKey]: 'CONVERSATION_RECOVERY_FAILED'
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-recovery-panel',
        message: `recoveryId=${item.recoveryId} revision=${item.revision} action=${action} code=CONVERSATION_RECOVERY_FAILED`
      })
    } finally {
      setRunning((current) => ({ ...current, [requestKey]: undefined }))
    }
  }

  return (
    <aside
      className={`${
        embedded
          ? 'relative w-full'
          : 'fixed inset-x-2 top-2 z-40 sm:inset-x-auto sm:right-3 sm:w-[34rem]'
      } max-h-[45vh] overflow-y-auto rounded-xl border border-amber-500/50 bg-background/95 p-3 shadow-lg backdrop-blur ${className ?? ''}`}
      aria-label={t('recovery.title')}
      data-conversation-recovery-panel=""
    >
      <div className="mb-3 flex items-start gap-2">
        <AlertTriangle className="mt-0.5 size-5 shrink-0 text-amber-600" aria-hidden="true" />
        <div>
          <h2 className="text-sm font-semibold">{t('recovery.title')}</h2>
          <p className="text-xs text-muted-foreground">{t('recovery.description')}</p>
        </div>
      </div>

      <div className="space-y-3">
        {visibleItems.map((item) => {
          const stateKey = actionStateKey(item.recoveryId, item.revision)
          const result = results[stateKey]
          const inspected = inspectedEvidence[stateKey]
          const errorCode = errors[stateKey]
          const activeAction = running[stateKey]
          const validatedInspection =
            inspected?.action === 'inspect' &&
            inspected.recoveryId === item.recoveryId &&
            inspected.recoveryRevision === item.revision
              ? inspected
              : undefined
          // Host status remains redacted. Only an exact, revision-correlated Inspect result becomes
          // a separate display authority; the original RecoveryItem is never merged or mutated.
          const immutablePaths = validatedInspection?.sourcePaths ?? item.sourcePaths
          const immutableChecksums = validatedInspection?.sourceSha256 ?? item.sourceSha256
          const immutableProvenance = validatedInspection?.provenance ?? item.provenance
          const immutableCandidateFacts = validatedInspection?.candidateFacts ?? item.candidateFacts
          const status = result?.status ?? item.status
          return (
            <section
              key={item.recoveryId}
              className="rounded-lg border border-border bg-card p-3"
              aria-label={`${item.kind} ${item.recoveryId}`}
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <p className="text-sm font-medium">{t(`recovery.kinds.${item.kind}`)}</p>
                  <p className="font-mono text-[10px] text-muted-foreground">
                    {item.recoveryId} ·{' '}
                    {t('recovery.revision', {
                      revision: result?.recoveryRevision ?? item.revision
                    })}
                  </p>
                </div>
                <span className="rounded-full bg-muted px-2 py-1 text-[10px] font-medium">
                  {t(`recovery.statuses.${status}` as const)}
                </span>
              </div>

              <dl className="mt-3 space-y-2 text-xs">
                <div>
                  <dt className="font-medium text-muted-foreground">{t('recovery.sources')}</dt>
                  <dd className="space-y-1">
                    {immutablePaths.map((path, index) => (
                      <code
                        key={`${path}-${immutableChecksums[index] ?? ''}`}
                        className="block break-all font-mono"
                      >
                        {path}
                        {immutableChecksums[index] ? ` · sha256:${immutableChecksums[index]}` : ''}
                      </code>
                    ))}
                  </dd>
                </div>
                <div>
                  <dt className="font-medium text-muted-foreground">{t('recovery.provenance')}</dt>
                  <dd className="space-y-1">
                    {immutableProvenance.map((entry) => (
                      <code
                        key={`${entry.sourceKind}:${entry.relativePath}`}
                        className="block break-all font-mono"
                      >
                        {entry.sourceKind} · {entry.relativePath} · sha256:{entry.sha256} ·{' '}
                        {t('recovery.preservedReadOnly')}
                      </code>
                    ))}
                  </dd>
                </div>
                {immutableCandidateFacts.length > 0 ? (
                  <div>
                    <dt className="font-medium text-muted-foreground">
                      {t('recovery.candidateFacts')}
                    </dt>
                    <dd className="space-y-1">
                      {immutableCandidateFacts.map((fact, index) => (
                        <code
                          key={`${item.recoveryId}-fact-${index}`}
                          className="block break-all font-mono"
                        >
                          {JSON.stringify(fact)}
                        </code>
                      ))}
                    </dd>
                  </div>
                ) : null}
              </dl>

              <div className="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-4">
                {item.suggestedActions.map((action) => (
                  <Button
                    key={action}
                    type="button"
                    variant={action === 'startEmptyWorkspace' ? 'default' : 'secondary'}
                    size="sm"
                    className="min-h-11 whitespace-normal px-2 text-xs"
                    disabled={Boolean(activeAction)}
                    aria-label={t(`recovery.actions.${action}` as const)}
                    data-recovery-action={action}
                    onClick={() => void run(item, action)}
                  >
                    {activeAction === action ? (
                      <LoaderCircle className="mr-1 size-3 animate-spin" aria-hidden="true" />
                    ) : null}
                    {t(`recovery.actions.${action}` as const)}
                  </Button>
                ))}
              </div>

              {result ? (
                <div
                  role="status"
                  aria-live="polite"
                  className="mt-3 flex items-center gap-2 rounded-md bg-emerald-500/10 px-2 py-1.5 text-xs text-emerald-700 dark:text-emerald-300"
                >
                  {result.workspaceChanged ? (
                    <CheckCircle2 className="size-4" aria-hidden="true" />
                  ) : (
                    <ShieldCheck className="size-4" aria-hidden="true" />
                  )}
                  {t('recovery.outcome', {
                    action: t(`recovery.actions.${result.action}` as const),
                    status: t(`recovery.statuses.${result.status}` as const),
                    workspaceRevision: result.workspaceRevision ?? t('recovery.unchanged')
                  })}
                </div>
              ) : null}
              {errorCode ? (
                <div
                  role="alert"
                  aria-live="assertive"
                  className="mt-3 rounded-md bg-destructive/10 px-2 py-1.5 text-xs text-destructive"
                  data-error-code={errorCode}
                >
                  {t(`errors.${errorCode}`, { defaultValue: errorCode })}
                </div>
              ) : null}
            </section>
          )
        })}
      </div>
    </aside>
  )
}
