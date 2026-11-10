/**
 * Attach/resume replay cursor that means "live only".
 *
 * Used when another channel has already been painting this PTY (a spawn
 * Channel, or the claim-less `watch` installed after a rehydrate). Replaying
 * the output log from an older cursor would write those bytes into xterm a
 * second time, so the host is asked to replay nothing and only stream what
 * happens next.
 *
 * Safe as JSON + Rust `u64`; PTY seqs never approach 2^53 in a session.
 */
export const LIVE_ONLY_LAST_SEQ = Number.MAX_SAFE_INTEGER
