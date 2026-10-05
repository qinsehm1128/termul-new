// Load the repro page in a real WKWebView (the engine Se ships on macOS) and
// save the PNG it reports. Usage: swift runner.swift <url> <out.png> <out.txt>
import AppKit
import WebKit

final class Runner: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
  let window: NSWindow
  let webView: WKWebView
  let pngPath: String
  let textPath: String

  init(url: URL, pngPath: String, textPath: String) {
    self.pngPath = pngPath
    self.textPath = textPath
    let config = WKWebViewConfiguration()
    let controller = WKUserContentController()
    config.userContentController = controller
    let frame = NSRect(x: 0, y: 0, width: 1000, height: 1400)
    webView = WKWebView(frame: frame, configuration: config)
    // Off-screen windows are occluded and WebKit stops rAF for occluded views.
    let occlusion = Selector(("_setWindowOcclusionDetectionEnabled:"))
    if webView.responds(to: occlusion) { webView.perform(occlusion, with: false) }
    window = NSWindow(contentRect: NSRect(x: -4000, y: -4000, width: 1000, height: 1400),
                      styleMask: [.borderless], backing: .buffered, defer: false)
    super.init()
    controller.add(self, name: "done")
    window.contentView = webView
    window.orderFrontRegardless()
    webView.load(URLRequest(url: url))
    DispatchQueue.main.asyncAfter(deadline: .now() + 120) {
      FileHandle.standardError.write("timeout\n".data(using: .utf8)!)
      exit(2)
    }
  }

  func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
    guard let body = message.body as? [String: Any] else { exit(3) }
    if let error = body["error"] as? String {
      FileHandle.standardError.write("page error: \(error)\n".data(using: .utf8)!)
      exit(4)
    }
    let dataURL = body["png"] as? String ?? ""
    let base64 = dataURL.components(separatedBy: ",").last ?? ""
    guard let data = Data(base64Encoded: base64) else { exit(5) }
    try? data.write(to: URL(fileURLWithPath: pngPath))
    let text = body["text"] as? String ?? ""
    try? text.write(toFile: textPath, atomically: true, encoding: .utf8)
    let w = body["width"] ?? 0, h = body["height"] ?? 0, dpr = body["dpr"] ?? 0, vy = body["viewportY"] ?? 0
    let atlas = body["atlas"] ?? ""
    print("ok canvas=\(w)x\(h) dpr=\(dpr) viewportY=\(vy) \(atlas)")
    exit(0)
  }
}

let args = CommandLine.arguments
guard args.count == 4, let url = URL(string: args[1]) else {
  print("usage: runner.swift <url> <out.png> <out.txt>")
  exit(1)
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let runner = Runner(url: url, pngPath: args[2], textPath: args[3])
app.run()
