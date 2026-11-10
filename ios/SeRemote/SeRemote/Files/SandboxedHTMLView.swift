import SwiftUI
import UIKit
import WebKit

/// Rendering policy for untrusted project HTML.
///
/// The document is loaded with `loadHTMLString(_, baseURL: nil)`, JavaScript is
/// disabled at the `WKWebpagePreferences` level, a strict CSP is injected at
/// the top of the document, and every navigation or subresource that is not
/// the local `about:blank`/`data:` surface is cancelled.
enum HTMLSandboxPolicy {
    nonisolated static let contentSecurityPolicy =
        "default-src 'none'; img-src data:; media-src data:; style-src 'unsafe-inline'; " +
        "font-src data:; script-src 'none'; connect-src 'none'; frame-src 'none'; " +
        "object-src 'none'; base-uri 'none'; form-action 'none'"

    /// Only the synthetic initial document and inline data URLs are local.
    /// Everything with a network, file, or script scheme is rejected.
    nonisolated static func allows(_ url: URL?) -> Bool {
        guard let url else { return true }
        switch url.scheme?.lowercased() {
        case "about":
            return url.absoluteString.lowercased() == "about:blank"
        case "data":
            return true
        default:
            return false
        }
    }

    /// Injects the CSP and a Nocturne text baseline immediately inside `<head>`
    /// so the policy applies before any author markup is parsed.
    nonisolated static func sanitizedDocument(
        _ html: String,
        textColor: String = "#F2F0E8",
        backgroundColor: String = "transparent"
    ) -> String {
        let injection = """
        <meta http-equiv="Content-Security-Policy" content="\(contentSecurityPolicy)">
        <style>
        html, body { background: \(backgroundColor); }
        body { color: \(textColor); font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif; \
        font-size: 15px; line-height: 1.5; margin: 14px; -webkit-text-size-adjust: 100%; overflow-wrap: break-word; }
        img, video { max-width: 100%; height: auto; }
        </style>
        """
        guard let head = html.range(of: "<head", options: .caseInsensitive),
              let headEnd = html[head.lowerBound...].firstIndex(of: ">") else {
            return injection + html
        }
        let insertAt = html.index(after: headEnd)
        return String(html[..<insertAt]) + injection + String(html[insertAt...])
    }

    nonisolated static func hex(_ color: UIColor) -> String {
        var red: CGFloat = 0
        var green: CGFloat = 0
        var blue: CGFloat = 0
        var alpha: CGFloat = 0
        guard color.cgColor.alpha >= 0.01 else { return "transparent" }
        guard color.getRed(&red, green: &green, blue: &blue, alpha: &alpha) else { return "#000000" }
        return String(
            format: "#%02X%02X%02X",
            Int((red * 255).rounded()),
            Int((green * 255).rounded()),
            Int((blue * 255).rounded())
        )
    }
}

/// Read-only HTML surface for the preview card.
struct SandboxedHTMLView: UIViewRepresentable {
    let html: String
    let textColor: UIColor
    let backgroundColor: UIColor

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.websiteDataStore = .nonPersistent()

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = false
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.underPageBackgroundColor = .clear
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        let document = HTMLSandboxPolicy.sanitizedDocument(
            html,
            textColor: HTMLSandboxPolicy.hex(textColor),
            backgroundColor: HTMLSandboxPolicy.hex(backgroundColor)
        )
        guard context.coordinator.lastDocument != document else { return }
        context.coordinator.lastDocument = document
        webView.loadHTMLString(document, baseURL: nil)
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var lastDocument: String?

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction
        ) async -> WKNavigationActionPolicy {
            HTMLSandboxPolicy.allows(navigationAction.request.url) ? .allow : .cancel
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationResponse: WKNavigationResponse
        ) async -> WKNavigationResponsePolicy {
            HTMLSandboxPolicy.allows(navigationResponse.response.url) ? .allow : .cancel
        }

        func webView(
            _ webView: WKWebView,
            createWebViewWith configuration: WKWebViewConfiguration,
            for navigationAction: WKNavigationAction,
            windowFeatures: WKWindowFeatures
        ) -> WKWebView? {
            nil
        }
    }
}
