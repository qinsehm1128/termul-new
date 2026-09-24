import {
  legacyUpdateComponentPolicy,
  type PendingUpdatePlan,
  UPDATE_COMPONENTS,
  type UpdateComponent,
  type UpdateComponentAction,
  type UpdateComponentPolicy
} from '@shared/types/updater.types'
import { describe, expect, it } from 'vitest'
import { i18n, initializeI18n } from '@/i18n'
import shellEn from '@/locales/en/shell.json'
import shellZh from '@/locales/zh-CN/shell.json'
import {
  buildUpdateInstallConfirmation,
  describeUpdateInstallSemantics,
  getMatchingCoreComponents,
  getPendingUpdateStatus,
  getUpdateComponentImpact,
  getUpdateImpactLines,
  getUpdateInstallMessageParts,
  presentPendingUpdate,
  translateUpdateCopy,
  type UpdateInstallSemantics
} from './updater-status'

function policyWith(
  actions: Partial<Record<UpdateComponent, UpdateComponentAction>>,
  buildPrefix = 'build'
): UpdateComponentPolicy {
  const base = legacyUpdateComponentPolicy('1.2.3')
  const components = { ...base.components }
  for (const component of UPDATE_COMPONENTS) {
    const action = actions[component] ?? components[component].action
    components[component] = { buildId: `${buildPrefix}-${component}`, action }
  }
  return { ...base, metadataState: 'declared', components }
}

function pendingPlan(
  status: PendingUpdatePlan['status'],
  overrides: Partial<PendingUpdatePlan> = {}
): PendingUpdatePlan {
  return {
    schemaVersion: 1,
    targetVersion: '1.2.3',
    componentPolicy: policyWith({}, 'policy-build'),
    status,
    currentComponentBuildIds: { acpCore: 'live-build-not-for-ui' },
    requiredActions: ['acpCore', 'terminalCore'],
    deferredComponents: [],
    createdAt: '2026-08-14T00:00:00.000Z',
    updatedAt: '2026-08-14T00:00:00.000Z',
    ...overrides
  }
}

function leafKeys(value: unknown, prefix = ''): string[] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [prefix]
  return Object.entries(value).flatMap(([key, child]) =>
    leafKeys(child, prefix ? `${prefix}.${key}` : key)
  )
}

function partKeys(semantics: UpdateInstallSemantics): string[] {
  return getUpdateInstallMessageParts(semantics).map((part) => part.key)
}

describe('updater status semantics', () => {
  it('filters preserve out of impact while still naming matching Cores', () => {
    const policy = policyWith({
      renderer: 'restart',
      guiNative: 'preserve',
      acpCore: 'preserve',
      terminalCore: 'restart'
    })

    expect(getUpdateComponentImpact(policy).map((entry) => entry.component)).toEqual([
      'renderer',
      'terminalCore'
    ])
    expect(getMatchingCoreComponents(policy)).toEqual(['acpCore'])
    const lines = getUpdateImpactLines(policy, translateUpdateCopy)
    expect(lines.join('\n')).toContain('Kept and reconnected because the build matches: ACP Core.')
    expect(lines.join('\n')).not.toContain('Desktop runtime')
    expect(lines.join('\n')).not.toContain('build-acpCore')
  })

  it('does not infer Core identity from build IDs or from active terminals alone', () => {
    const actions = {
      renderer: 'restart',
      guiNative: 'restart',
      acpCore: 'preserve',
      terminalCore: 'defer-if-active'
    } satisfies Partial<Record<UpdateComponent, UpdateComponentAction>>
    const active = {
      policy: policyWith(actions, 'alpha'),
      hasActiveTerminalSessions: true
    }
    expect(describeUpdateInstallSemantics(active)).toEqual(
      describeUpdateInstallSemantics({
        policy: policyWith(actions, 'beta'),
        hasActiveTerminalSessions: true
      })
    )

    const missing = describeUpdateInstallSemantics({
      policy: null,
      hasActiveTerminalSessions: true
    })
    expect(missing).toMatchObject({
      guiRestarts: true,
      guiRelaunchKeepsCorePty: true,
      reconnectNotZeroInterruption: true,
      metadataUnavailable: true,
      matchingCores: [],
      mismatchedRestartCores: [],
      terminalDeferred: false,
      terminalReplaceNow: false
    })
    expect(partKeys(missing)).not.toContain('updates.installSemantics.terminalDefer')
  })

  it('distinguishes matching adoption, mismatched restart, and active-PTY defer', () => {
    const matching = describeUpdateInstallSemantics({
      policy: policyWith({
        renderer: 'preserve',
        guiNative: 'preserve',
        acpCore: 'preserve',
        terminalCore: 'preserve'
      }),
      hasActiveTerminalSessions: true
    })
    expect(matching.guiRestarts).toBe(true)
    expect(matching.matchingCores).toEqual(['acpCore', 'terminalCore'])
    expect(matching.terminalDeferred).toBe(false)
    expect(partKeys(matching)).toContain('updates.installSemantics.coreAdopt')
    expect(partKeys(matching)).not.toContain('updates.installSemantics.terminalDefer')

    const deferred = describeUpdateInstallSemantics({
      policy: policyWith({ acpCore: 'restart', terminalCore: 'defer-if-active' }),
      hasActiveTerminalSessions: true
    })
    expect(deferred.mismatchedRestartCores).toEqual(['acpCore'])
    expect(deferred.terminalDeferred).toBe(true)
    expect(deferred.terminalReplaceNow).toBe(false)
    expect(partKeys(deferred)).toEqual([
      'updates.installSemantics.guiRestart',
      'updates.installSemantics.ptySurvivesGui',
      'updates.installSemantics.coreRestart',
      'updates.installSemantics.terminalDefer',
      'updates.installSemantics.reconnect'
    ])

    const idle = describeUpdateInstallSemantics({
      policy: policyWith({ acpCore: 'preserve', terminalCore: 'defer-if-active' }),
      hasActiveTerminalSessions: false
    })
    expect(idle.terminalDeferred).toBe(false)
    expect(idle.terminalReplaceNow).toBe(true)
    expect(partKeys(idle)).toContain('updates.installSemantics.terminalReplaceIdle')
    expect(partKeys(idle)).not.toContain('updates.installSemantics.terminalDefer')

    const immediate = describeUpdateInstallSemantics({
      policy: policyWith({ acpCore: 'preserve', terminalCore: 'restart' }),
      hasActiveTerminalSessions: true
    })
    expect(immediate.terminalDeferred).toBe(false)
    expect(immediate.terminalRestartDespiteActive).toBe(true)
    expect(immediate.mismatchedRestartCores).toEqual(['terminalCore'])
    const confirmation = buildUpdateInstallConfirmation({
      policy: policyWith({ acpCore: 'preserve', terminalCore: 'restart' }),
      hasActiveTerminalSessions: true,
      version: '1.2.3',
      translate: translateUpdateCopy
    })
    expect(confirmation).toContain('The app window always restarts.')
    expect(confirmation).toContain(
      'Restarting the app window itself does not stop terminals owned by Terminal Core.'
    )
    expect(confirmation).toContain(
      'Terminal Core build identity is checked. If it differs, immediate replacement may stop the terminals it owns instead of waiting.'
    )
    expect(confirmation).toContain('Kept and reconnected because the build matches: ACP Core.')
    expect(confirmation).toContain(
      'Terminal Core build identity is checked. If it differs, immediate replacement may stop the terminals it owns instead of waiting.'
    )
    expect(confirmation).not.toContain('Replacement waits while a terminal is active.')
    expect(confirmation).not.toContain('policy-build')
    expect(confirmation).not.toContain('build-terminalCore')
  })

  it('presents durable plan states and keeps lastError without live build ids', () => {
    expect(getPendingUpdateStatus(pendingPlan('prepared'))).toBeNull()
    expect(getPendingUpdateStatus(null)).toBeNull()

    const failed = presentPendingUpdate(
      pendingPlan('failed', { lastError: '  reconnect timed out  ' }),
      translateUpdateCopy
    )
    expect(failed).toMatchObject({
      status: 'failed',
      offerRetry: true,
      lastError: 'reconnect timed out',
      summary: 'Saved update plan failed. Review the error, then check again.'
    })
    expect(failed?.label).toBe(
      'Saved update plan failed. Review the error, then check again. reconnect timed out'
    )
    expect(failed?.label).not.toContain('live-build-not-for-ui')
    expect(failed?.label).not.toContain('policy-build-acpCore')

    expect(
      presentPendingUpdate(pendingPlan('failed', { lastError: '   ' }), translateUpdateCopy)
    ).toMatchObject({ lastError: null, offerRetry: true })
    expect(presentPendingUpdate(pendingPlan('reconciling'), translateUpdateCopy)?.offerRetry).toBe(
      false
    )
    expect(presentPendingUpdate(pendingPlan('completed'), translateUpdateCopy)).toMatchObject({
      status: 'completed',
      offerRetry: false,
      summary: 'Saved update plan finished. Matching Cores stayed running and were reconnected.'
    })
    expect(
      presentPendingUpdate(
        pendingPlan('deferred', { deferredComponents: ['terminalCore'] }),
        translateUpdateCopy
      )?.summary
    ).toBe(
      'Saved update plan: replacement of Terminal Core is waiting because a terminal is still active. Restarting the app window does not stop that terminal.'
    )
    expect(
      presentPendingUpdate(pendingPlan('deferred'), translateUpdateCopy)?.summary
    ).not.toContain('Terminal Core')
  })

  it('keeps English and Chinese update keys aligned and translates the GUI restart rule', async () => {
    expect(leafKeys(shellZh.updates).sort()).toEqual(leafKeys(shellEn.updates).sort())
    expect(leafKeys(shellZh.updateReadyModal).sort()).toEqual(
      leafKeys(shellEn.updateReadyModal).sort()
    )
    expect(shellEn.updates.aurAvailable).toBe('A new version is available. Update with yay.')
    expect(shellEn.updates.manualChannel).toBe(
      'A new {{channel}} build is available. Open the download page to install it manually.'
    )
    expect(shellEn.updates.useYay).toBe('Use yay')
    expect(shellEn.updates.downloadPage).toBe('Open Download Page')
    expect(shellEn.updates.errors.aurSelfUpdate).toBe(
      'AUR build cannot self-update. Update with: {{command}}'
    )
    expect(shellEn.updates.errors.aurSelfInstall).toBe(
      'AUR build cannot self-install updates. Update with: {{command}}'
    )
    expect(shellZh.updates.aurAvailable).toBe('有新版本可用。请使用 yay 更新。')
    expect(shellZh.updates.manualChannel).toBe(
      '有新的 {{channel}} 构建可用。请打开下载页面手动安装。'
    )
    expect(shellZh.updates.useYay).toBe('使用 yay')
    expect(shellZh.updates.downloadPage).toBe('打开下载页面')
    expect(shellZh.updates.errors.aurSelfUpdate).toBe('AUR 构建无法自行更新。请运行：{{command}}')
    expect(shellZh.updates.errors.aurSelfInstall).toBe(
      'AUR 构建无法自行安装更新。请运行：{{command}}'
    )

    await initializeI18n('zh-CN')
    expect(i18n.t('updates.installSemantics.guiRestart', { ns: 'shell' })).toBe(
      '应用窗口总会重启。'
    )
    expect(i18n.t('updates.installSemantics.reconnect', { ns: 'shell' })).toBe(
      '被替换的 Core 会通过重新连接接管。这不是零中断切换。'
    )
  })
})
