/**
 * The aggregator must install *every* boot diagnostic.
 *
 * `main.web-bundle.test.ts` proves both entry points call
 * `installBootInstrumentation()`; this proves the call is worth making. Without
 * it, dropping one of the two installers here would reintroduce the original
 * defect — instrumentation present in the bundle, never running — with the
 * entry-point guard still green.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'

const installGlobalErrorForwarding = vi.fn()
const installRendererLifecycleMarkers = vi.fn()

vi.mock('./lib/log-api', () => ({
  installGlobalErrorForwarding: () => installGlobalErrorForwarding()
}))

vi.mock('./lib/renderer-lifecycle-markers', () => ({
  installRendererLifecycleMarkers: () => installRendererLifecycleMarkers()
}))

describe('installBootInstrumentation', () => {
  beforeEach(() => {
    installGlobalErrorForwarding.mockClear()
    installRendererLifecycleMarkers.mockClear()
  })

  it('installs error forwarding and the renderer lifecycle markers', async () => {
    const { installBootInstrumentation } = await import('./boot-instrumentation')

    installBootInstrumentation()

    expect(installGlobalErrorForwarding).toHaveBeenCalledTimes(1)
    expect(installRendererLifecycleMarkers).toHaveBeenCalledTimes(1)
  })
})
