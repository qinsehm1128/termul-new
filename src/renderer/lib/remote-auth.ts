import { logFrontendError } from './log-api'
import { getRemoteAccessCredential, setRemoteAccessCredential } from './remote-access-credential'

function browserDeviceName(): string {
  const platform = (navigator.platform ?? '')
    .replace(/[^a-zA-Z0-9 ._-]/g, '')
    .trim()
    .slice(0, 64)
  const uaPlatform = navigator.userAgent.match(/Android|iPhone|iPad|Windows|Mac|Linux/i)?.[0]
  return `${platform || uaPlatform || 'Web'} 浏览器`
}

/**
 * Must resolve before importing App or constructing authenticated HTTP/WS clients.
 * A one-time code is never persisted: remove its fragment before the first await,
 * exchange it without an Authorization header, and keep only the device token in memory.
 */
export async function initializeRemoteAuth(): Promise<void> {
  const fragment = new URLSearchParams(window.location.hash.replace(/^#/, ''))
  if (!fragment.has('pair')) {
    getRemoteAccessCredential()
    return
  }
  const code = fragment.get('pair') ?? ''
  window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`)
  // Do not fall back to a stale/legacy credential if the exchange fails.
  setRemoteAccessCredential('')
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 15000)
  try {
    if (!code) throw new Error('Empty pairing code')
    const response = await fetch(`${window.location.origin}/pair`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ code, name: browserDeviceName(), platform: 'web' }),
      signal: controller.signal
    })
    if (!response.ok) throw new Error('Pairing exchange rejected')
    const payload: unknown = await response.json()
    if (
      !payload ||
      typeof payload !== 'object' ||
      !('deviceToken' in payload) ||
      typeof payload.deviceToken !== 'string' ||
      !payload.deviceToken
    )
      throw new Error('Invalid pairing response')
    setRemoteAccessCredential(payload.deviceToken)
    void logFrontendError({
      level: 'warn',
      source: 'remote-auth',
      message: 'browser pairing completed'
    })
  } catch {
    void logFrontendError({ source: 'remote-auth', message: 'browser pairing failed' })
    throw new Error('配对失败，请重新扫码')
  } finally {
    clearTimeout(timer)
  }
}
