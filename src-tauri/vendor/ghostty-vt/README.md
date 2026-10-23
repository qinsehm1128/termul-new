# Pinned Ghostty VT source

- Upstream: https://github.com/ghostty-org/ghostty
- Ghostty commit: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`
- Version string: `1.3.2-HEAD-+44f2a44df`
- Transport source: `herdr` commit `5bd900d88c80d95f5b145397e45bbeea398b8165`,
  path `vendor/libghostty-vt`, tree `e5ec12a4a8178f5f32f60407760e5c2d2a4883b9`.
- Archive: `ghostty-vt-44f2a44df-source.tar.gz`
- SHA256: `af8cd2fbd25c17990b25c8bf8352263c35c5ec446134220de867236497311edd`
- Size: 3,939,039 bytes; 1,339 regular files under `ghostty-vt/`.
- License: upstream Ghostty VT **MIT**, copied verbatim to `LICENSE`.

The deterministic archive contains only the pinned VT source tree. No herdr
Apache-licensed Rust bindings are included or used. The sibling crate implements
its own tiny C shim against the upstream C headers, without bindgen/libclang.
The native build verifies the digest before extraction, rejects links and unsafe
paths, enforces extraction size/file-count limits, and builds inside `OUT_DIR`.
The default Rust build neither reads/extracts this archive nor invokes Zig.

Native Zig dependency downloads remain governed by the hashes in the pinned
`build.zig.zon`. A first native build may need network access. Failures retain
Zig's exact dependency/fetch diagnostics rather than substituting a fake engine.
