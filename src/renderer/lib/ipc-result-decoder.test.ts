import { decodeIpcResult as sharedDecodeIpcResult } from '@shared/types/ipc.types'
import { expect, it } from 'vitest'
import { decodeIpcResult as compatibilityDecodeIpcResult } from './ipc-result-decoder'

it('re-exports the shared decodeIpcResult function identity without parsing', () => {
  expect(compatibilityDecodeIpcResult).toBe(sharedDecodeIpcResult)
  const envelope = { success: true as const, data: { value: 'exact' } }
  expect(compatibilityDecodeIpcResult(envelope, (value) => value as { value: string })).toBe(
    envelope
  )
  expect(() =>
    compatibilityDecodeIpcResult({ success: true, data: {}, extra: true }, (value) => value)
  ).toThrow(TypeError)
})
