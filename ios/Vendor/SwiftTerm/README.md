# SwiftTerm (vendored)

Upstream: [migueldeicaza/SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) `1.20.0`.

Xcode 27 rejects the upstream `SwiftTermBuildInfoPlugin` during plug-in validation. This tree is the iOS library only:

- no build-tool plugin
- static `SwiftTermBuildInfo` stub
- Metal shader copied as source (runtime compile), so CLI builds do not need the Metal toolchain
- tiny `AccessibilityService` stub (upstream keeps that type in the Mac folder)

Do not replace this with the GitHub package until that plugin validates on the current Xcode.
