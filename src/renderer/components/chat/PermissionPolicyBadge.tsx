import { ShieldCheck, ShieldQuestion } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import type { PermissionPolicy } from '@/lib/acp-api'
import { cn } from '@/lib/utils'
import type { AcpSession } from '@/stores/acp-store'
import { useAcpStore } from '@/stores/acp-store'

interface PermissionPolicyBadgeProps {
  session: AcpSession
}

function reuseKeyConfigId(key: string): string {
  const separator = key.indexOf('\0')
  return separator === -1 ? key : key.slice(0, separator)
}

export function PermissionPolicyBadge({
  session
}: PermissionPolicyBadgeProps): React.JSX.Element | null {
  const { t } = useTranslation('chat')
  const saveAgentConfig = useAcpStore((state) => state.saveAgentConfig)
  const agentConfig = useAcpStore((state) => {
    const liveEntry = Object.entries(state.configToLiveAgent ?? {}).find(
      ([, agentId]) => agentId === session.agentId
    )
    if (!liveEntry) return undefined
    const configId = reuseKeyConfigId(liveEntry[0])
    return (state.agentConfigs ?? []).find(
      (config) => config.id === configId || config.configId === configId
    )
  })
  const [saving, setSaving] = useState(false)
  const [confirmAllowAll, setConfirmAllowAll] = useState(false)

  if (!agentConfig) return null

  const policy = agentConfig.permissionPolicy ?? 'ask'
  const allowAll = policy === 'allow_all'

  const updatePolicy = async (nextPolicy: PermissionPolicy): Promise<void> => {
    setSaving(true)
    try {
      await saveAgentConfig({ ...agentConfig, permissionPolicy: nextPolicy })
    } catch (error) {
      toast.error(
        t('permissionPolicy.updateFailed', {
          defaultValue: 'Could not update tool permissions: {{message}}',
          message: error instanceof Error ? error.message : String(error)
        })
      )
    } finally {
      setSaving(false)
    }
  }

  return (
    <>
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            className={cn(
              'inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-2xs transition-colors',
              'text-muted-foreground hover:bg-muted hover:text-foreground',
              allowAll && 'text-amber-500 hover:text-amber-500'
            )}
            aria-label={t('permissionPolicy.open', {
              defaultValue: 'Tool permission policy: {{policy}}',
              policy: allowAll
                ? t('permissionPolicy.allowAllShort', { defaultValue: 'Full access' })
                : t('permissionPolicy.askShort', { defaultValue: 'Ask' })
            })}
          >
            {allowAll ? <ShieldCheck size={13} /> : <ShieldQuestion size={13} />}
            <span>
              {allowAll
                ? t('permissionPolicy.allowAllShort', { defaultValue: 'Full access' })
                : t('permissionPolicy.askShort', { defaultValue: 'Ask' })}
            </span>
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" side="top" className="w-72 p-3">
          <div className="space-y-3">
            <div>
              <p className="text-sm font-medium text-foreground">
                {t('permissionPolicy.title', { defaultValue: 'Tool permissions' })}
              </p>
              <p className="mt-1 text-xs text-muted-foreground">
                {t('permissionPolicy.description', {
                  defaultValue:
                    'Choose whether this ACP agent must ask before using tools. This applies to all conversations using the agent.'
                })}
              </p>
            </div>
            <div className="flex items-start justify-between gap-3 rounded-md border border-border/60 p-2.5">
              <div>
                <p className="text-xs font-medium text-foreground">
                  {t('permissionPolicy.allowAll', { defaultValue: 'Allow all tool requests' })}
                </p>
                <p className="mt-0.5 text-2xs text-muted-foreground">
                  {t('permissionPolicy.allowAllHint', {
                    defaultValue: 'Automatically accepts allow options offered by the agent.'
                  })}
                </p>
              </div>
              <Switch
                checked={allowAll}
                disabled={saving}
                aria-label={t('permissionPolicy.toggle', {
                  defaultValue: 'Allow all tool requests'
                })}
                onCheckedChange={(checked) => {
                  if (checked) setConfirmAllowAll(true)
                  else void updatePolicy('ask')
                }}
              />
            </div>
          </div>
        </PopoverContent>
      </Popover>

      <AlertDialog open={confirmAllowAll} onOpenChange={setConfirmAllowAll}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t('permissionPolicy.confirmTitle', {
                defaultValue: 'Allow all requests from this agent?'
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t('permissionPolicy.confirmDescription', {
                defaultValue:
                  'This agent will be able to use every allow option it presents without asking each time. It may modify files or execute commands. Only enable this for an agent and workspace you trust.'
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {t('permissionPolicy.keepAsking', { defaultValue: 'Keep asking' })}
            </AlertDialogCancel>
            <AlertDialogAction
              className="bg-amber-600 text-white hover:bg-amber-700"
              onClick={() => void updatePolicy('allow_all')}
            >
              {t('permissionPolicy.enableFullAccess', { defaultValue: 'Enable full access' })}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
