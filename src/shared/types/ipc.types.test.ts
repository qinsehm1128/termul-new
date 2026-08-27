import { expect, it, vi } from 'vitest'
import { decodeIpcResult } from './ipc.types'

it('decodes exact success and failure IPC envelopes and rejects malformed shapes', () => {
  const data = { value: 7 }
  const success = { success: true as const, data }
  const decodeData = vi.fn((value: unknown) => {
    expect(value).toBe(data)
    return value as typeof data
  })
  expect(decodeIpcResult(success, decodeData)).toBe(success)
  expect(decodeData).toHaveBeenCalledTimes(1)

  const failure = { success: false as const, error: 'conflict', code: 'CONFLICT' }
  const failureDecoder = vi.fn()
  expect(decodeIpcResult(failure, failureDecoder)).toBe(failure)
  expect(failureDecoder).not.toHaveBeenCalled()

  const transformed = decodeIpcResult({ success: true, data: '7' }, (value) => Number(value))
  expect(transformed).toEqual({ success: true, data: 7 })

  const malformed: unknown[] = [
    null,
    [],
    {},
    { success: 'true', data },
    { success: true },
    { success: true, data, extra: true },
    { success: true, data, error: 'contradictory' },
    { success: false, error: '', code: 'E' },
    { success: false, error: ' ', code: 'E' },
    { success: false, error: 'bad', code: '' },
    { success: false, error: 'bad', code: ' ' },
    { success: false, error: 1, code: 'E' },
    { success: false, error: 'bad', code: 1 },
    { success: false, error: 'bad', code: 'E', data: null }
  ]
  for (const value of malformed) {
    expect(() => decodeIpcResult(value, (item) => item)).toThrow(TypeError)
  }

  const validator = vi.fn(() => {
    throw new TypeError('invalid domain data')
  })
  expect(() => decodeIpcResult({ success: true, data }, validator)).toThrow('invalid domain data')
  expect(validator).toHaveBeenCalledTimes(1)
})
