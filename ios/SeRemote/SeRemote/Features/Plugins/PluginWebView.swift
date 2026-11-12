import SwiftUI
import UIKit
import WebKit

/// The remote-access web app as a live surface.
///
/// This is deliberately not `SandboxedHTMLView`: plugin pages need JavaScript,
/// network access and same-origin iframes. `PluginBrowserPolicy` is what keeps
/// the frame on the paired host, and the non-persistent data store keeps the
/// session — and its credential — out of the app's shared website data.
struct PluginWebView: UIViewRepresentable {
    let bootstrapURL: URL
    let origin: URL
    let backgroundColor: UIColor
    @Binding var progress: Double
    @Binding var isLoading: Bool
    @Binding var loadFailed: Bool

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.allowsInlineMediaPlayback = true
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.isOpaque = false
        webView.backgroundColor = backgroundColor
        webView.scrollView.backgroundColor = backgroundColor
        webView.underPageBackgroundColor = backgroundColor
        webView.allowsBackForwardNavigationGestures = false
        context.coordinator.observe(webView)
        webView.load(URLRequest(url: bootstrapURL))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        // Reloads replace the view (`.id(attempt)`), so only the bindings need
        // refreshing here.
        context.coordinator.parent = self
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var parent: PluginWebView
        private var observations: [NSKeyValueObservation] = []

        init(_ parent: PluginWebView) {
            self.parent = parent
        }

        func observe(_ webView: WKWebView) {
            observations = [
                webView.observe(\.estimatedProgress, options: [.new]) { [weak self] webView, _ in
                    let value = webView.estimatedProgress
                    Task { @MainActor [weak self] in self?.parent.progress = value }
                },
                webView.observe(\.isLoading, options: [.new]) { [weak self] webView, _ in
                    let value = webView.isLoading
                    Task { @MainActor [weak self] in self?.parent.isLoading = value }
                },
            ]
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction
        ) async -> WKNavigationActionPolicy {
            let isMainFrame = navigationAction.targetFrame?.isMainFrame ?? false
            let decision = PluginBrowserPolicy.decide(
                url: navigationAction.request.url,
                isMainFrame: isMainFrame,
                origin: parent.origin
            )
            switch decision {
            case .allow:
                return .allow
            case .openExternally(let url):
                UIApplication.shared.open(url, options: [:], completionHandler: nil)
                return .cancel
            case .cancel:
                return .cancel
            }
        }

        func webView(
            _ webView: WKWebView,
            createWebViewWith configuration: WKWebViewConfiguration,
            for navigationAction: WKNavigationAction,
            windowFeatures: WKWindowFeatures
        ) -> WKWebView? {
            guard let url = navigationAction.request.url else { return nil }
            switch PluginBrowserPolicy.decide(url: url, isMainFrame: true, origin: parent.origin) {
            case .allow:
                webView.load(URLRequest(url: url))
            case .openExternally(let external):
                UIApplication.shared.open(external, options: [:], completionHandler: nil)
            case .cancel:
                break
            }
            return nil
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
            parent.isLoading = true
            parent.progress = 0
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            parent.loadFailed = false
            parent.isLoading = false
            parent.progress = 1
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            report(error)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            report(error)
        }

        private func report(_ error: Error) {
            let failure = error as NSError
            // A cancelled navigation is the policy sending a link to the system
            // browser, not a page that failed to load.
            guard failure.code != NSURLErrorCancelled else { return }
            HostLog.ui.error("plugins load failed: \(failure.domain, privacy: .public) \(failure.code)")
            parent.loadFailed = true
            parent.isLoading = false
        }
    }
}
