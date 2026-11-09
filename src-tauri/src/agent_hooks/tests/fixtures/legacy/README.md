# Frozen legacy hook renderer bytes (A0)

These fixtures capture `install.rs` before the adapter redesign. They are legacy
ownership/upgrade compatibility evidence, not a specification for a new renderer.

## Capture contract

- Fake home: `/se-hooks-legacy-fixture/home 'with spaces` (including the apostrophe).
- Renderers receive that home explicitly via `Adapter::new(..., PathBuf, false)`.
  No process `HOME` mutation, CLI discovery, endpoint read, or installation occurs.
- Outputs were captured directly from `script_source`, `pi_v1_source`, `pi_source`,
  and `merge`. They have **not** been normalized, reformatted, or path-substituted.
- `legacy_renderers_reproduce_exact_committed_bytes` reads these files and compares
  raw bytes. It never rewrites fixtures. Preserve trailing newlines and the empty
  `qin-config.empty.removed.toml` file.

## Coverage

- Three v1 shell scripts: Claude Code, qin-code, and pi.
- Exact pi extension v1 and v2; direct merge checks also freeze v1-to-v2 upgrade,
  v2 idempotency, and removal of either owned version.
- Claude empty settings at every distinct event gate: 1.0.23, 1.0.31, 1.0.41,
  1.0.53, 1.0.62, 2.0.43, 2.0.45, 2.0.56, and 2.1.261. Tests check the version
  immediately before each later gate against its preceding fixture, plus unknown,
  unparseable, and multiline version outputs. Versions below 1.0.23 are rejected.
  Unknown versions use the same bytes as 1.0.62 (the conservative 1.0.64 baseline).
- Claude representative user settings, preserving unrelated fields and user hooks:
  install, idempotency, removal with unknown/latest versions, and latest-to-unknown
  downgrade (including a user-owned SessionEnd hook).
- qin empty config and representative user config with comments, explicit hook
  level, legacy hook field, and a user rule: install, idempotency, and removal.

The `*.input.*` files are user-owned configuration examples, not renderer outputs.
All other JSON/TOML/SH/TS files are exact renderer or merge results. Do not refresh
legacy fixtures to make a redesigned renderer pass; retain legacy recognition.
