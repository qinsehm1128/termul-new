import { decodeIpcResult } from '@shared/types/ipc.types'
import { beforeEach, expect, it, vi } from 'vitest'
import { HTTP_IPC_NETWORK_ERROR_MESSAGE } from './http-ipc-result'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { invokeDecodedIpcResult } from './invoke-decoded-ipc-result'

beforeEach(() => {
  invokeMock.mockReset()
})

it('invokeDecodedIpcResult uses decodeIpcResult and domain parsers', async () => {
  const exactData = { hostKind: 'desktop' }
  const envelope = { success: true as const, data: exactData }
  const parse = vi.fn((value: unknown) => {
    if (value === exactData) return value as typeof exactData
    throw new TypeError('invalid domain payload')
  })

  invokeMock.mockResolvedValueOnce(envelope)
  const success = await invokeDecodedIpcResult('conversation_host_status', parse)
  expect(success).toBe(envelope)
  expect(success).toEqual(decodeIpcResult(envelope, parse))
  expect(parse).toHaveBeenCalledTimes(2)
  expect(parse).toHaveBeenCalledWith(exactData)
  expect(invokeMock).toHaveBeenCalledWith('conversation_host_status', undefined)

  invokeMock.mockResolvedValueOnce({ success: true, data: exactData, extra: true })
  await expect(invokeDecodedIpcResult('conversation_host_status', parse)).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })

  invokeMock.mockResolvedValueOnce({ success: true, data: { secret: 'body' } })
  await expect(invokeDecodedIpcResult('conversation_list', parse)).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
  expect(parse).toHaveBeenCalledWith({ secret: 'body' })

  invokeMock.mockRejectedValueOnce(new TypeError('IPC result must be an object'))
  await expect(invokeDecodedIpcResult('conversation_get', parse)).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
})
