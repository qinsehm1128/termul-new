/**
 * Closed-contract helpers shared by the AI, MCP facade, and navigation types.
 * Error text is static and must never echo a rejected value.
 */

export const CONTRACT_MAX_SAFE_INTEGER = Number.MAX_SAFE_INTEGER

const FORBIDDEN_CREDENTIAL_KEYS = new Set([
  'apikey',
  'apisecret',
  'secret',
  'token',
  'bearer',
  'bearertoken',
  'accesstoken',
  'refreshtoken',
  'password',
  'passwd',
  'authorization',
  'clientsecret',
  'idtoken',
  'privatekey',
  'credential',
  'credentials',
  'rawkey',
  'sessiontoken'
])

export function contractFail(message: string): never {
  throw new Error(message)
}

export function isForbiddenCredentialKey(key: string): boolean {
  return FORBIDDEN_CREDENTIAL_KEYS.has(key.toLowerCase().replace(/[_-]/g, ''))
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function readClosedObject(
  value: unknown,
  allowed: readonly string[],
  invalidMessage: string,
  forbiddenMessage: string
): Record<string, unknown> {
  if (!isRecord(value)) contractFail(invalidMessage)
  for (const key of Object.keys(value)) {
    if (isForbiddenCredentialKey(key)) contractFail(forbiddenMessage)
    if (!allowed.includes(key)) contractFail(invalidMessage)
  }
  return value
}

export function rejectNulls(
  value: unknown,
  invalidMessage: string,
  forbiddenMessage: string,
  depth = 0
): void {
  if (depth > 32) contractFail(invalidMessage)
  if (value === null) contractFail(invalidMessage)
  if (typeof value === 'string') {
    if (value.length > 100_000) contractFail(invalidMessage)
    return
  }
  if (Array.isArray(value)) {
    if (value.length > 10_000) contractFail(invalidMessage)
    for (const item of value) rejectNulls(item, invalidMessage, forbiddenMessage, depth + 1)
    return
  }
  if (isRecord(value)) {
    if (Object.keys(value).length > 256) contractFail(invalidMessage)
    for (const [key, item] of Object.entries(value)) {
      if (isForbiddenCredentialKey(key)) contractFail(forbiddenMessage)
      rejectNulls(item, invalidMessage, forbiddenMessage, depth + 1)
    }
  }
}

export function hasOwn(record: Record<string, unknown>, key: string): boolean {
  return Object.hasOwn(record, key)
}

export function requireBoolean(value: unknown, message: string): boolean {
  if (typeof value !== 'boolean') contractFail(message)
  return value
}

export function requireInteger(value: unknown, min: number, max: number, message: string): number {
  if (typeof value !== 'number' || !Number.isInteger(value) || value < min || value > max) {
    contractFail(message)
  }
  return value
}

export function isSafeId(value: string, maxLength: number): boolean {
  return value.length > 0 && value.length <= maxLength && /^[A-Za-z][A-Za-z0-9._-]*$/.test(value)
}

export function requireSafeId(value: unknown, maxLength: number, message: string): string {
  if (typeof value !== 'string' || !isSafeId(value, maxLength)) contractFail(message)
  return value
}

export function requireModelId(value: unknown, maxLength: number, message: string): string {
  if (typeof value !== 'string') contractFail(message)
  if (value.length === 0 || value.length > maxLength) contractFail(message)
  if (!/^[A-Za-z0-9][A-Za-z0-9_.:/-]*$/.test(value)) contractFail(message)
  return value
}

export function requireCredentialRef(value: unknown, maxLength: number, message: string): string {
  if (typeof value !== 'string') contractFail(message)
  if (value.length === 0 || value.length > maxLength) contractFail(message)
  if (!/^[A-Za-z0-9][A-Za-z0-9_./:-]*$/.test(value)) contractFail(message)
  return value
}

export function requireDisplayText(
  value: unknown,
  maxChars: number,
  message: string,
  options: { allowEmpty?: boolean; allowNewlines?: boolean } = {}
): string {
  if (typeof value !== 'string') contractFail(message)
  const chars = Array.from(value)
  if (chars.length > maxChars) contractFail(message)
  if (!options.allowEmpty && chars.length === 0) contractFail(message)
  if (value !== value.trim()) contractFail(message)
  for (const char of chars) {
    const code = char.codePointAt(0) ?? 0
    const newline = options.allowNewlines && (code === 9 || code === 10)
    if (!newline && (code < 32 || code === 127)) contractFail(message)
  }
  return value
}

export function utf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).length
}

export function isLoopbackHostname(hostname: string): boolean {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, '')
  return host === 'localhost' || host === '127.0.0.1' || host === '::1'
}

export function assertSafeHttpUrl(value: string, maxLength: number, message: string): void {
  if (value.length === 0 || value.length > maxLength || /\s/u.test(value)) contractFail(message)
  let url: URL
  try {
    url = new URL(value)
  } catch {
    contractFail(message)
  }
  if (url.username !== '' || url.password !== '' || url.hash !== '') contractFail(message)
  const loopback = isLoopbackHostname(url.hostname)
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) contractFail(message)
  if (url.hostname === '') contractFail(message)
  for (const key of url.searchParams.keys()) {
    if (isForbiddenCredentialKey(key)) contractFail(message)
  }
}
