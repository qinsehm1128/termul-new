import { describe, expect, it } from 'vitest'
import {
  CANARY_IDENTIFIER,
  CANARY_PRODUCT_NAME,
  CANARY_UPDATE_MANIFEST_URL,
  canaryEnvironment,
  resolveCanaryVersion
} from './canary-profile.mjs'

describe('Canary build profile', () => {
  it('uses a stable side-by-side identity and non-production updater manifest', () => {
    expect(CANARY_IDENTIFIER).toBe('com.se-manager.app.canary')
    expect(CANARY_PRODUCT_NAME).toBe('Se Manager Canary')
    expect(CANARY_UPDATE_MANIFEST_URL).toContain('/releases/download/canary/')
    expect(CANARY_UPDATE_MANIFEST_URL).not.toContain('/releases/latest/')
  })

  it('derives a valid prerelease version without changing package.json', () => {
    expect(resolveCanaryVersion('0.12.3')).toBe('0.12.3-canary.1')
    expect(resolveCanaryVersion('0.12.3', '0.12.3-canary.7')).toBe('0.12.3-canary.7')
    expect(() => resolveCanaryVersion('0.12.3', 'canary')).toThrow(/Invalid Canary version/)
  })

  it('marks both Rust and Vite builds as Canary', () => {
    const environment = canaryEnvironment('0.12.3-canary.1')
    expect(environment.SE_CANARY_BUILD).toBe('1')
    expect(environment.VITE_SE_CANARY).toBe('1')
    expect(environment.VITE_APP_VERSION_OVERRIDE).toBe('0.12.3-canary.1')
  })
})
