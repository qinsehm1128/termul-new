/**
 * The remote-access bearer credential, shared by every authenticated HTTP and
 * WebSocket client in the web build. Kept apart from any one transport so
 * features that only need the header do not depend on the agent transport.
 */

/** Process-memory-only credential shared by authenticated HTTP and WebSocket transports. */
let remoteAccessCredential: string | null = null
let remoteAccessCredentialConsumed = false

/**
 * Consume the QR-delivered credential fragment exactly once. Fragments are not
 * sent to the server; clearing it immediately keeps the credential out of
 * browser history updates, query parameters, storage, and later copied URLs.
 */
export function getRemoteAccessCredential(): string {
  if (remoteAccessCredentialConsumed) return remoteAccessCredential ?? ''
  if (typeof window === 'undefined') return ''

  remoteAccessCredentialConsumed = true
  const rawHash = window.location.hash.startsWith('#')
    ? window.location.hash.slice(1)
    : window.location.hash
  const fragment = new URLSearchParams(rawHash)
  remoteAccessCredential = fragment.get('access_token')
  if (fragment.has('access_token')) {
    window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`)
  }
  return remoteAccessCredential ?? ''
}

/** Add the in-memory bearer credential without persisting or logging it. */
export function remoteAccessHeaders(initial?: HeadersInit): Headers {
  const headers = new Headers(initial)
  const credential = getRemoteAccessCredential()
  if (credential) headers.set('authorization', `Bearer ${credential}`)
  return headers
}

/** @internal test helper */
export function _resetRemoteAccessCredentialForTests(): void {
  remoteAccessCredential = null
  remoteAccessCredentialConsumed = false
}

/** Forget the credential for good; the user must pair again. */
export function revokeRemoteAccessCredential(): void {
  remoteAccessCredential = null
  remoteAccessCredentialConsumed = true
}
