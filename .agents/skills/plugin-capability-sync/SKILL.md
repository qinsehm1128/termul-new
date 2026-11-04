---
name: plugin-capability-sync
description: Keep the plugin capability catalog in sync with Se Manager features. Use whenever you add, change, rename or remove a Tauri command (generate_handler! in src-tauri/src/lib.rs), a web/standalone API route, a backend event (emit), or renderer-owned user state (project/settings/theme/shortcut/layout stores). Not needed for pure refactors that keep command names, inputs, outputs and events unchanged.
---

# Plugin capability sync

Se Manager exposes its features to plugins through one capability catalog. Every app capability is open to plugins **except credentials and secret values** (user decision, 2026-11-04). When the app gains or changes a capability, the catalog must change in the same commit, so plugins never fall behind and a secret never leaks by accident.

- Catalog (source of truth): `.qin-code/docs/specs/plugin-capability-catalog-spec.md`
- Platform plan: `.qin-code/docs/plans/plugin-platform-plan.md` (Design A–G, packages C1–C4)
- Coverage check: `python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py`

## When this applies

Run this workflow if your change does any of the following:

1. Adds, renames, removes, or changes the input/output of a `#[tauri::command]` registered in `generate_handler!`.
2. Adds or changes a web route in `src-tauri/src/web/` or `server_main.rs` that has no Tauri twin.
3. Adds or changes an `emit(...)` event, or a renderer event topic.
4. Adds user-meaningful state that only the renderer owns (a new store or a new field in the project, settings, theme, shortcut, or layout stores).
5. Starts storing, reading or returning a credential, token, key, password, or env/header value.

## Steps

1. **Find the row.** Search the catalog for the command name. New commands have no row yet.
2. **Classify it** with the rules below, then write or update one row. Use the columns of the part the domain belongs to (A: agents/sessions/conversations/memory/skills/scheduled/ai/mcp/remote/plugins; B: terminal/quick/git/worktree/search/fs/workspace/ssh/sftp/tunnel; C: browser/data/system/logs plus renderer-owned).
   - `op id`: `se.<domain>.<verb>` in camelCase.
   - source: `command_name \`path:line\``. Keep the raw command name in the row, because the checker matches on it.
   - `class`: R, W, D (destructive or irreversible), or SEC.
   - `scope`: profile, project, session, terminal, sshConnection, pluginSelf, or global.
   - `confirm`: none, grant, or per-call.
   - `exposure`: OPEN, OPEN-REDACTED, SECRET-EXCLUDED, or INTERNAL.
   - notes: input and output in one line, long-running yes/no, and gotchas.
3. **Update the event table** when you added or changed an event. Events carry revision and ids only, never document bodies or secrets.
4. **Update the counts** in the `### 统计` table at the top of the catalog.
5. **Update the code registry if it exists.** Once packages C1–C3 have landed, a catalog row alone is not enough. Also update:
   - the operation descriptor and the provider adapter under `src-tauri/src/capability_host/`;
   - the projection tests: one success test, one out-of-scope test, a per-call test without confirmation, and a sentinel no-leak test for OPEN-REDACTED.
   
   Before C3 lands, the catalog row is the whole obligation.
6. **Run the check** and fix every `UNCLASSIFIED` line:
   ```bash
   python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py
   ```
7. **Commit the catalog with the feature.** `.qin-code/` and `.agents/` are git-ignored, so stage them with `git add -f <path>`.

## Classification rules

| Situation | exposure | confirm |
|---|---|---|
| The op exists to read or write a secret value: API key, SSH password or private key, askpass, OAuth or bearer token, secure-storage value, terminal claim credential, webview storage handoff | SECRET-EXCLUDED | n/a |
| The op uses a stored credential but never returns it (SSH connect, `git push`, AI channel test, MCP OAuth complete) | OPEN | by risk |
| The op returns or echoes data that contains a secret (MCP config env/headers, connection info with a bearer, remote status with a token, raw logs) | OPEN-REDACTED: return `configured`, `hasToken`, or `authenticated` booleans instead | by risk |
| Deletes data, force-pushes, writes raw terminal input, kills a process, approves an agent tool call, installs native code or an agent, rotates a credential, exposes the app on the network | OPEN | per-call |
| Changes persistent state with a normal, reversible effect | OPEN | grant |
| Read-only | OPEN | none |
| UI plumbing with no user-meaningful capability (refs, visibility toggles, menus, internal callbacks) | INTERNAL: name the covering op in notes | n/a |

Hard rules:

- Never mark a whole feature SECRET-EXCLUDED because it touches credentials. Only the secret-valued op is excluded.
- Any op that takes a path, cwd, terminalId, sessionId, or conversationId must be resolved and checked on the host against the caller's granted scope. If the existing command skips that check, write it in the notes as a gap. The catalog `## 不变量` section lists the known gaps.
- Read-only ops must not migrate, index, or write to disk.
- Long-running ops return a job id and report progress through events. Add a cancel op if one is missing.
- Desktop, shared-live remote, standalone `se-server`, and browser must expose the same op. If an op is platform-only, say so in the notes and specify what the unsupported platforms return.

## Done when

- The checker prints `unclassified: 0`.
- Every changed command or event has an up-to-date row, and the `### 统计` counts match.
- No OPEN or OPEN-REDACTED row returns a secret value.
- After C3: the registry descriptor and the four projection tests exist and pass.
