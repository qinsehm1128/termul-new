import {
  isScreenObservationReady,
  legacyScreenObservation
} from '@shared/types/terminal-screen.types'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { logFrontendError } from '@/lib/log-api'
import { readTerminalScreenStatus } from '@/lib/terminal-api'
import { useScreenObservationStore } from '@/stores/screen-observation-store'
import { useTerminalStore } from '@/stores/terminal-store'

/** User-initiated status only: no timer, no screen content, no preference-derived ready state. */
export function TerminalScreenObservationStatus({ terminalId }: { terminalId?: string }) {
  const { t } = useTranslation('terminal')
  const terminal = useTerminalStore((state) =>
    state.terminals.find((item) => item.id === terminalId || item.ptyId === terminalId)
  )
  const ptyId = terminal?.ptyId ?? terminalId
  const host = useScreenObservationStore((state) => (ptyId ? state.byPtyId[ptyId] : undefined))
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  const request = useRef(0)
  // biome-ignore lint/correctness/useExhaustiveDependencies: resource generation changes must invalidate in-flight replies, even without a new request.
  useEffect(() => {
    ++request.current
    setBusy(false)
    setFailed(false)
    return () => {
      ++request.current
    }
  }, [ptyId, terminal?.claim])
  const status = host?.status ?? legacyScreenObservation()
  const state = failed
    ? 'unsupported'
    : status.state === 'ready' && !isScreenObservationReady(status)
      ? 'degraded'
      : status.state

  async function refresh() {
    if (!ptyId || busy) return
    const serial = ++request.current
    const claim = terminal?.claim ?? ''
    setBusy(true)
    setFailed(false)
    try {
      const result = await readTerminalScreenStatus(ptyId, claim)
      const current = useTerminalStore.getState().terminals.find((item) => item.id === terminal?.id)
      if (
        serial !== request.current ||
        (terminal && (current?.ptyId !== ptyId || current.claim !== terminal.claim))
      )
        return
      if (result.success) {
        useScreenObservationStore.getState().recordHost(ptyId, {
          terminalScreenEngine: result.data.terminalScreenEngine,
          status: result.data
        })
      } else {
        setFailed(true)
        void logFrontendError({
          level: 'warn',
          source: 'host-screen-status',
          message: 'Host screen status unavailable'
        })
      }
    } catch {
      if (serial === request.current) setFailed(true)
      void logFrontendError({
        level: 'warn',
        source: 'host-screen-status',
        message: 'Host screen status transport failed'
      })
    } finally {
      if (serial === request.current) setBusy(false)
    }
  }

  if (!ptyId) return null
  return (
    <details className="absolute bottom-1 right-2 z-30 max-w-xs rounded border border-border/60 bg-background/95 px-2 py-1 text-2xs text-muted-foreground shadow-sm">
      <summary className="cursor-pointer select-none">
        {t('screenObservation.label')}
        {host?.terminalScreenEngine === 'ghostty-vt'
          ? ` · ${t(`screenObservation.states.${state}`)}`
          : ''}
      </summary>
      <div className="mt-2 space-y-1.5" role="status">
        <p>{t('screenObservation.description')}</p>
        <p>
          {t('screenObservation.engine')}:{' '}
          {host?.terminalScreenEngine ?? status.terminalScreenEngine}
        </p>
        <p>
          {t('screenObservation.state')}:{' '}
          {t(`screenObservation.states.${state}`, {
            defaultValue: t('screenObservation.states.unsupported')
          })}
        </p>
        {status.reason && (
          <p>
            {t(`screenObservation.reasons.${status.reason}`, {
              defaultValue: t('screenObservation.unavailable')
            })}
          </p>
        )}
        <p>
          {t('screenObservation.sequence')}: {status.appliedSeq}/{status.capturedSeq}
        </p>
        {failed && <p>{t('screenObservation.unavailable')}</p>}
        <button
          type="button"
          disabled={busy}
          onClick={() => void refresh()}
          className="rounded border border-border px-2 py-1 disabled:opacity-50"
        >
          {busy ? t('screenObservation.refreshing') : t('screenObservation.refresh')}
        </button>
      </div>
    </details>
  )
}
