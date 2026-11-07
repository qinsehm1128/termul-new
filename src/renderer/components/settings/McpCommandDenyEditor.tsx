import type { McpCommandDenyRule, McpHostAccessPolicy } from '@shared/types/mcp-host.types'
import { useId, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

export const MAX_COMMAND_DENY_RULES = 32
export const MAX_TOTAL_COMMAND_DENY_RULES = 128
export const MAX_COMMAND_DENY_PATTERN_BYTES = 512

// Structural checks only. Rust owns regex compilation and command matching.
function validPatternText(pattern: string): boolean {
  return (
    pattern.length > 0 &&
    !pattern.includes('\0') &&
    new TextEncoder().encode(pattern).length <= MAX_COMMAND_DENY_PATTERN_BYTES
  )
}

export function validCommandDenyRules(rules: McpCommandDenyRule[]): boolean {
  const ids = new Set<string>()
  return (
    rules.length <= MAX_COMMAND_DENY_RULES &&
    rules.every((rule) => {
      if (
        rule.id.length === 0 ||
        rule.id.length > 128 ||
        ![...rule.id].every((char) =>
          'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.:'.includes(char)
        ) ||
        ids.has(rule.id) ||
        !['exact', 'contains', 'regex'].includes(rule.kind) ||
        !validPatternText(rule.pattern)
      )
        return false
      ids.add(rule.id)
      return true
    })
  )
}

export function validCommandDenyPolicy(policy: McpHostAccessPolicy): boolean {
  const layers = [
    policy.ssh.commandDeny ?? [],
    ...Object.values(policy.ssh.profiles).map((grant) => grant.commandDeny ?? [])
  ]
  return (
    layers.every(validCommandDenyRules) &&
    layers.reduce((count, rules) => count + rules.length, 0) <= MAX_TOTAL_COMMAND_DENY_RULES
  )
}

interface Props {
  rules: McpCommandDenyRule[]
  onChange?: (rules: McpCommandDenyRule[]) => void
  disabled?: boolean
  inherited?: boolean
  title: string
}

export function McpCommandDenyEditor({ rules, onChange, disabled, inherited, title }: Props) {
  const { t } = useTranslation('mcp')
  const id = useId()
  const [pattern, setPattern] = useState('')
  const [kind, setKind] = useState<McpCommandDenyRule['kind']>('exact')
  const [advanced, setAdvanced] = useState(false)
  const locked = disabled || inherited || !onChange
  const validPattern = validPatternText(pattern)
  const options = (showRegex: boolean) => (
    <>
      <option value="exact">{t('commandDeny.exact')}</option>
      <option value="contains">{t('commandDeny.contains')}</option>
      {showRegex && <option value="regex">{t('commandDeny.regex')}</option>}
    </>
  )
  const update = (index: number, patch: Partial<McpCommandDenyRule>) => {
    onChange?.(rules.map((rule, i) => (i === index ? { ...rule, ...patch } : rule)))
  }
  return (
    <fieldset disabled={!!locked} className="min-w-0 space-y-2 rounded-md border p-3">
      <legend className="px-1 text-sm font-medium">
        {title} ({rules.length}/32)
      </legend>
      <p className="text-xs text-muted-foreground">
        {t(inherited ? 'commandDeny.inherited' : 'commandDeny.help')}
      </p>
      {rules.length === 0 && (
        <p className="text-xs text-muted-foreground">{t('commandDeny.empty')}</p>
      )}
      <ul className="space-y-2">
        {rules.map((rule, index) => (
          <li key={rule.id} className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <select
                className="h-8 max-w-full rounded-md border bg-background px-2 text-xs"
                aria-label={t('commandDeny.ruleKind', { index: index + 1 })}
                value={rule.kind}
                onChange={(event) =>
                  update(index, { kind: event.target.value as McpCommandDenyRule['kind'] })
                }
              >
                {options(advanced || rule.kind === 'regex')}
              </select>
              <Input
                className="h-8 min-w-0 flex-1 basis-40 font-mono text-xs"
                aria-label={t('commandDeny.rulePattern', { index: index + 1 })}
                aria-invalid={!validCommandDenyRules([rule])}
                aria-describedby={!validCommandDenyRules([rule]) ? `${id}-invalid` : undefined}
                value={rule.pattern}
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => update(index, { pattern: event.target.value })}
              />
              {!inherited && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={t('commandDeny.removeRule', { index: index + 1 })}
                  onClick={() => onChange?.(rules.filter((_, i) => i !== index))}
                >
                  {t('commandDeny.remove')}
                </Button>
              )}
            </div>
            <span className="break-all font-mono text-[10px] text-muted-foreground">{rule.id}</span>
          </li>
        ))}
      </ul>
      {!validCommandDenyRules(rules) && (
        <p id={`${id}-invalid`} role="alert" className="text-xs text-destructive">
          {t('commandDeny.invalid')}
        </p>
      )}
      {!inherited && (
        <>
          <label className="flex items-center gap-2 text-xs">
            <input
              type="checkbox"
              checked={advanced}
              onChange={(event) => {
                setAdvanced(event.target.checked)
                if (!event.target.checked && kind === 'regex') setKind('exact')
              }}
            />
            {t('commandDeny.advanced')}
          </label>
          {advanced && (
            <p className="text-xs text-muted-foreground">{t('commandDeny.regexHelp')}</p>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <select
              className="h-8 max-w-full rounded-md border bg-background px-2 text-xs"
              aria-label={t('commandDeny.newKind')}
              value={kind}
              onChange={(event) => setKind(event.target.value as McpCommandDenyRule['kind'])}
            >
              {options(advanced)}
            </select>
            <Input
              className="h-8 min-w-0 flex-1 basis-40 font-mono text-xs"
              aria-label={t('commandDeny.newPattern')}
              value={pattern}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setPattern(event.target.value)}
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={!validPattern || rules.length >= MAX_COMMAND_DENY_RULES}
              onClick={() => {
                onChange?.([...rules, { id: crypto.randomUUID(), kind, pattern }])
                setPattern('')
              }}
            >
              {t('commandDeny.add')}
            </Button>
          </div>
          {pattern.length > 0 && !validPattern && (
            <p role="alert" className="text-xs text-destructive">
              {t('commandDeny.invalid')}
            </p>
          )}
        </>
      )}
    </fieldset>
  )
}
