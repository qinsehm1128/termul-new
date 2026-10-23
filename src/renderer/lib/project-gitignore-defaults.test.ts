import { __resetBrandCanonicalOverride, __setBrandCanonicalOverride } from '@shared/brand'
import { describe, expect, it } from 'vitest'
import { getProjectGitignoreDefaults } from './project-gitignore-defaults'

describe('project gitignore defaults', () => {
  it('preserves compatibility rules and the active workspace directory without duplicates', () => {
    __setBrandCanonicalOverride({ workspaceDir: '.se-manager' })
    expect(getProjectGitignoreDefaults()).toBe('.qin-code/\n.se-manage/\n.se-manager/\n')
    __resetBrandCanonicalOverride()
  })

  it('includes the canary workspace directory when the active brand is canary', () => {
    __setBrandCanonicalOverride({ workspaceDir: '.se-manager-canary' })
    expect(getProjectGitignoreDefaults()).toContain('.se-manager-canary/')
    __resetBrandCanonicalOverride()
  })
})
