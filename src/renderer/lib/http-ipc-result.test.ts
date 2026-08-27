import { afterEach, expect, it, vi } from 'vitest'
import {
  decodeHttpIpcResult,
  HTTP_IPC_NETWORK_ERROR_MESSAGE,
  requestHttpIpcResult
} from './http-ipc-result'

function response(
  body: unknown,
  status: number
): { response: Response; json: ReturnType<typeof vi.fn> } {
  const json = vi.fn(async () => body)
  return {
    response: { status, ok: status >= 200 && status < 300, json } as unknown as Response,
    json
  }
}

afterEach(() => vi.unstubAllGlobals())

it('decodes one exact IPC envelope for 200 409 422 and 500 with one JSON parse', async () => {
  for (const status of [200, 409, 422, 500]) {
    const successBody = { success: true as const, data: { status, value: 'ok' } }
    const successResponse = response(successBody, status)
    const parser = vi.fn((value: unknown) => value as { status: number; value: string })
    await expect(decodeHttpIpcResult(successResponse.response, parser)).resolves.toBe(successBody)
    expect(successResponse.json).toHaveBeenCalledTimes(1)
    expect(parser).toHaveBeenCalledTimes(1)

    const failureBody = {
      success: false as const,
      error: `failure-${status}`,
      code: `CODE_${status}`
    }
    const failureResponse = response(failureBody, status)
    const unusedParser = vi.fn()
    await expect(decodeHttpIpcResult(failureResponse.response, unusedParser)).resolves.toBe(
      failureBody
    )
    expect(failureResponse.json).toHaveBeenCalledTimes(1)
    expect(unusedParser).not.toHaveBeenCalled()
  }

  const malformedBodies: unknown[] = [
    null,
    [],
    {},
    { success: 'true', data: {} },
    { success: true },
    { success: true, data: {}, extra: true },
    { success: false, error: '', code: 'E' },
    { success: false, error: 'bad', code: '' },
    { success: false, error: 'bad', code: 'E', data: {} }
  ]
  for (const body of malformedBodies) {
    const malformed = response(body, 500)
    await expect(decodeHttpIpcResult(malformed.response, (value) => value)).resolves.toEqual({
      success: false,
      error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
      code: 'NETWORK_ERROR'
    })
    expect(malformed.json).toHaveBeenCalledTimes(1)
  }

  const invalidJson = {
    status: 200,
    ok: true,
    json: vi.fn(async () => {
      throw new SyntaxError('body must not escape')
    })
  } as unknown as Response
  await expect(decodeHttpIpcResult(invalidJson, (value) => value)).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
  expect(invalidJson.json).toHaveBeenCalledTimes(1)

  const invalidDomain = response({ success: true, data: { secret: 'body' } }, 200)
  const domainParser = vi.fn(() => {
    throw new TypeError('invalid domain payload')
  })
  await expect(decodeHttpIpcResult(invalidDomain.response, domainParser)).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
  expect(invalidDomain.json).toHaveBeenCalledTimes(1)
  expect(domainParser).toHaveBeenCalledTimes(1)

  const fetchMock = vi
    .fn()
    .mockResolvedValueOnce(response({ success: true, data: 7 }, 422).response)
    .mockRejectedValueOnce(new Error('/sensitive/path token=hidden'))
  vi.stubGlobal('fetch', fetchMock)
  await expect(requestHttpIpcResult('/ok', undefined, (value) => Number(value))).resolves.toEqual({
    success: true,
    data: 7
  })
  const rejected = await requestHttpIpcResult('/rejected', undefined, (value) => value)
  expect(rejected).toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
  expect(JSON.stringify(rejected)).not.toContain('/sensitive/path')
})
