# Se iOS companion

Native SwiftUI client for the desktop shared-live session. It does **not** run a PTY or tunnel sidecar. The desktop still hosts agents and terminals; this app pairs over HTTPS and speaks the same `/ws`, `/terminal/ws`, `/projects`, and `/fs/*` contracts as the browser client.

The pairing chrome is native (home, language, appearance). After you connect, **Chat**, **Terminal**, project switching, and a read-only file tree are native too. There is no WebView.

## Open in Xcode

1. Open `ios/TermulRemote/TermulRemote.xcodeproj`.
2. Select your Development Team for signing (`com.se-manager.remote`).
3. Run on a physical iPhone to use the camera QR scanner. Simulator can paste the copied link.

Requires Xcode 26 and iOS 26.

## Pairing

1. On the desktop, enable remote access in the status bar.
2. Scan the QR, or paste the copied `https://…` link.
3. After pairing, choose **Sessions** (independent chats) or **Projects** (desktop project terminals). The access URL fragment (`#access_token=…`) is the bearer credential.

Deep link: `se://open?url=<percent-encoded-access-url>`. Encode the `#access_token` fragment inside `url`, or pass `access_token` as a query item. A raw `#` on the `se://` URL is recovered as the bearer. The pre-rename `termul` scheme is no longer registered or accepted, so a link saved outside the app before the rename has to be re-copied; pairing itself is unaffected, since the QR and the copy button both hand out an `https://…` access URL.

Quick Tunnel (`*.trycloudflare.com`) still goes through Cloudflare even on the same Wi-Fi. After an iPhone restart the first open waits for the network and retries. RFC1918 / `.local` HTTP origins are allowed for a true LAN pair.

ACP agents that are already running on the Mac, or whose CLI is on the Mac PATH (`cursor-agent`, Codex via npx), can be selected from the phone. Switching reuses the live host process instead of starting a second one. Agents that advertise sign-in open that flow on the computer.

Terminal lists the host’s **already running** PTYs for the active desktop session (`list` by `conversationId`, then `watch`) and shows their scrollback. “New terminal” is optional and conversation-scoped. Opening the terminal tab on the phone takes the live PTY size, parks the desktop FitAddon, and restores that parked size when the phone leaves, disconnects, or switches back to Desktop. Pinch or A-/A+ still scales local text (50%–200%). Opening the keyboard lifts the input dock and leaves host geometry alone. The emulator is [SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) (`ios/Vendor/SwiftTerm`, plugin stripped so Xcode 27 can compile it).

## Language

Settings → Language: system, English, or Simplified Chinese.
