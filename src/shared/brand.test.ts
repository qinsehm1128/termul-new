/**
 * T-H01 — the brand seam itself.
 *
 * Everything in Wave 1 rests on one property: a test can make `brandCanonical()`
 * return the *post*-rename value while production still emits the pre-rename
 * one. Without it, a harness test would be asserting a constant against a copy
 * of itself and could never go red.
 */
import { afterEach, describe, expect, it } from 'vitest'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from './brand'

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('brand seam', () => {
  it('returns the shipped canonical values by default', () => {
    expect(brandCanonical().createdBy).toBe('termul')
    expect(brandCanonical().workspaceDir).toBe('.termul')
  })

  it('returns injected values while an override is in force', () => {
    __setBrandCanonicalOverride({ createdBy: 'se-manager' })
    expect(brandCanonical().createdBy).toBe('se-manager')
  })

  it('leaves un-overridden fields at their shipped values', () => {
    __setBrandCanonicalOverride({ createdBy: 'se-manager' })
    expect(brandCanonical().planFence).toBe('se-plan')
  })

  it('restores the shipped values when the override is cleared', () => {
    __setBrandCanonicalOverride({ createdBy: 'se-manager' })
    __resetBrandCanonicalOverride()
    expect(brandCanonical().createdBy).toBe('termul')
  })

  it('never lets an override reach the LEGACY values', () => {
    // LEGACY is what is already on users' disks. If the seam could move it,
    // every compatibility-read path would shift under the migration's feet.
    __setBrandCanonicalOverride({ createdBy: 'se-manager', workspaceDir: '.se-manager' })
    expect(LEGACY.createdBy).toBe('termul')
    expect(LEGACY.workspaceDir).toBe('.termul')
  })

  it('exposes a LEGACY value for every canonical key', () => {
    // A canonical key with no legacy twin is a contract whose migration path
    // was never written down.
    expect(Object.keys(LEGACY).sort()).toEqual(Object.keys(brandCanonical()).sort())
  })
})
