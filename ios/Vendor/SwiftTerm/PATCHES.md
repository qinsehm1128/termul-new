# Local patches against upstream SwiftTerm 1.20.0

Base: https://github.com/migueldeicaza/SwiftTerm @ 1.20.0 (see README.md for the
plugin-strip reason this vendored copy exists). Beyond the build-system strip,
these source files carry small local changes. Re-apply this list after any
upstream refresh:

| File | Change | Reason |
|------|--------|--------|
| `Sources/SwiftTerm/Apple/AppleTerminalView.swift` | dropped tautological `cellDimension != nil` in `glyphSlotFit` guard | warning cleanup; CellDimension (CGSize) is non-optional |
| `Sources/SwiftTerm/Apple/Metal/MetalTerminalRenderer.swift` | `_ =` discard on two `vertices.withUnsafeBytes` memcpy blocks | warning cleanup; result was already ignored semantically |
| `Sources/SwiftTerm/iOS/iOSTerminalView.swift` | `var`→`let` in accessibility helpers (startingLine/endingLine/rect/verticalWidth/start/end), removed dead `lineWidth`/`text` bindings, dropped redundant `?? false` after non-optional Bool (`terminalAccessory?.controlModifier ?? controlModifier`), `let items` in showContextMenu | warning cleanup only; zero behavior change |
| `Sources/SwiftTerm/iOS/iOSTextInput.swift` | explicit `String(describing:)` interpolation for optional `_markedTextRange` log | warning cleanup |

All four files were made writable (`chmod u+w`) for these edits; the rest of
the tree stays read-only by convention. Nothing here changes terminal
semantics, input handling, or rendering output.
