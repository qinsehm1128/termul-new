import type { LegacyConversationSourceKind } from '@shared/types/conversation-api.types'
import { AlertTriangle, LoaderCircle } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'
import { conversationRouteErrorKey } from '@/components/conversation/ConversationRoute'
import { conversationApi } from '@/lib/conversation-api'
import { logFrontendError } from '@/lib/log-api'

interface ChatRouteProps {
  sourceKind?: LegacyConversationSourceKind
  value?: string
}

/** Read-only compatibility route that resolves an opaque legacy key to ConversationId. */
export function ChatRoute({
  sourceKind = 'legacyAgentSessionId',
  value
}: ChatRouteProps = {}): React.JSX.Element | null {
  const { t } = useTranslation('common')
  const params = useParams<{ legacyValue: string }>()
  const navigate = useNavigate()
  const legacyValue = value ?? params.legacyValue ?? ''
  const [errorCode, setErrorCode] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    if (!legacyValue) {
      setLoading(false)
      return
    }
    let active = true
    setLoading(true)
    setErrorCode(null)
    void conversationApi
      .resolveLegacyConversationId({ sourceKind, value: legacyValue })
      .then((result) => {
        if (!active) return
        if (!result.success) {
          setErrorCode(result.code)
          setLoading(false)
          void logFrontendError({
            level: 'warn',
            source: 'legacy-conversation-route',
            message: `sourceKind=${sourceKind} code=${result.code}`
          })
          return
        }
        const canonicalPath = result.data.canonicalRoute.replace(/^#/, '')
        navigate(canonicalPath, { replace: true })
      })
      .catch(() => {
        if (!active) return
        setErrorCode('CONVERSATION_LEGACY_RESOLVE_FAILED')
        setLoading(false)
        void logFrontendError({
          level: 'warn',
          source: 'legacy-conversation-route',
          message: `sourceKind=${sourceKind} code=CONVERSATION_LEGACY_RESOLVE_FAILED`
        })
      })
    return () => {
      active = false
    }
  }, [legacyValue, navigate, sourceKind])

  if (loading) {
    return (
      <div
        role="status"
        aria-live="polite"
        className="absolute inset-x-3 top-3 z-20 flex items-center gap-2 rounded-md border bg-background/95 px-3 py-2 text-sm shadow-sm"
      >
        <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
        {t('conversationRoute.resolvingLegacy')}
      </div>
    )
  }

  if (!errorCode) return null
  return (
    <div
      role="alert"
      aria-live="assertive"
      className="absolute inset-x-3 top-3 z-20 flex items-start gap-2 rounded-md border border-destructive bg-background/95 p-3 text-sm text-destructive shadow-sm"
      data-error-code={errorCode}
    >
      <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <div>
        <p className="font-medium">{t('conversationRoute.legacyError')}</p>
        <p className="mt-1 text-xs">{t(conversationRouteErrorKey(errorCode))}</p>
        <code className="mt-1 block font-mono text-[10px] opacity-80">{errorCode}</code>
      </div>
    </div>
  )
}
