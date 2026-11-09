# Hooks contract — A0 frozen

Source of truth: `src/shared/types/agent-hooks.types.ts` and `src-tauri/src/agent_hooks/contract.rs`.
Wire fields are snake_case; timestamps are Unix milliseconds; revisions/generations are nonnegative safe JS integers. Nullable response fields serialize as null; optional sync cursors may be omitted. Adapter IDs match `^[a-z][a-z0-9-]{0,63}$`; claude-code/pi/qin-code stay reserved.

## A ↔ B transport

Final Tauri commands and HTTP equivalents (all JSON bodies use the named DTO directly; Tauri passes it as `{ request }`):

| Command | HTTP | Input → output |
|---|---|---|
| `agent_hooks_status_cmd` | GET `/api/agent-hooks/status` | none → `AdapterStatusDto[]` |
| `agent_hooks_reload_cmd` | POST `/api/agent-hooks/reload` | none → `AdapterStatusDto[]` |
| `agent_hooks_probe_cmd` | POST `/api/agent-hooks/probe` | `{ adapter_id }` → `AdapterStatusDto` |
| `agent_hooks_preview_cmd` | POST `/api/agent-hooks/preview` | `HookPreviewRequest` → `HookPreviewDto` |
| `agent_hooks_confirm_cmd` | POST `/api/agent-hooks/confirm` | `HookConfirmRequest` → `HookMutationResult` |
| `agent_hooks_install_cmd` | POST `/api/agent-hooks/install` | `HookConfirmRequest` → `HookMutationResult` (install preview only) |
| `agent_hooks_uninstall_cmd` | POST `/api/agent-hooks/uninstall` | `HookConfirmRequest` → `HookMutationResult` (uninstall preview only) |
| `agent_hooks_open_folder_cmd` | no HTTP route | none → void; browser facade returns `UNSUPPORTED_PLATFORM` |
| `agent_hooks_state_cmd` | GET `/api/agent-hooks/state?instance_id=…&since_revision=…` | `HookStateSyncRequest` → `HookStateSyncResponse` |
| `agent_hooks_resume_plan_cmd` | POST `/api/agent-hooks/resume-plan` | `ResumePlanRequest` → `ResumePlanDto` |
| `agent_hooks_resume_spawn_cmd` | POST `/api/agent-hooks/resume-spawn` | `ResumeSpawnRequest` → `ResumeSpawnResult` |

`confirm` executes the preview's action; install/uninstall are action-checked aliases. Upgrade is explicit `action: "install"`. Event options are native-event-name → boolean, omitted names use manifest defaults. No startup/settings/reload path may install or probe; opening settings may reload definitions. No plugin confirmation flow: mutations keep existing manage-only local-host authorization; paired devices get 403. Preserve legacy unsuffixed commands/exports only until callers migrate; they are not the new facade.

Push event: **`agent-hooks-state`**, payload `HookStateSyncResponse` with `kind: "delta"` (not raw events). Snapshot replaces the authorized view; its tombstones are empty. Delta upserts states and removes `(instance_id, terminal_id)` tombstones. Always advance to `current_revision`; project-filtered gaps are legal. Instance mismatch/expired cursor returns snapshot. B owns the only reconcile timer; C consumes B's subscription, never polls independently.

Errors: `HookErrorDto { code, message }`; `VALIDATION_ERROR`→400, `FORBIDDEN`→403, `CONFLICT`/`IDENTITY_STALE`/`PLAN_EXPIRED`→409, `UNSUPPORTED_PLATFORM`/`UNSUPPORTED_AGENT_CHAT`→422, `INTERNAL_ERROR`→500. Messages/logs must not contain secrets, raw payloads, full paths or argv.

## A ↔ C Rust service boundary

C implements the following names/signatures in its owned modules; A invokes them from verified hook ingress and transport wiring. `contract::*` means `crate::agent_hooks::contract::*`. C owns any internal descriptor/claim representation; it must not change these public DTOs. These are declarations for implementation, not claims that services already exist.

```rust
// cli_session::identity_store
pub fn upsert_verified_identity(
    home: &std::path::Path,
    owner_session_id: &str,
    state: &contract::HookStateDto,
    snapshot_ref: &str,
    variant: &str,
) -> Result<contract::HookIdentityDto, contract::HookErrorDto>;

pub fn mark_ended(
    home: &std::path::Path,
    descriptor_id: &str,
    expected_generation: u64,
) -> Result<(), contract::HookErrorDto>;

// cli_session::restore_service
pub struct RestoreContext<'a> {
    pub home: &'a std::path::Path,
    pub host_instance_id: &'a str,
    pub project_id: &'a str,
    pub owner_session_id: &'a str,
    pub automatic: bool,
    pub auto_resume_enabled: bool,
    pub terminal_service: &'a crate::core::TerminalServiceHandle,
}

pub async fn resume_plan(
    context: &RestoreContext<'_>,
    request: contract::ResumePlanRequest,
) -> Result<contract::ResumePlanDto, contract::HookErrorDto>;

pub async fn resume_spawn(
    context: &RestoreContext<'_>,
    request: contract::ResumeSpawnRequest,
) -> Result<contract::ResumeSpawnResult, contract::HookErrorDto>;
```

Context, HOME, owner, snapshot_ref and variant are host-derived, never trusted request fields. A resolves project/terminal permission before calling; C rechecks descriptor ownership, generation, liveness, setting and trusted snapshot. Identity upsert receives ownership-validated starts/identity changes only; equal identity must not write. It returns the persisted descriptor reference and host-generated display argv. A calls mark_ended only for a verified normal session_end; C handles interrupted/mismatch/10s confirmation lifecycle without killing PTYs. Descriptor/claim locking is cross-process; spawn idempotency key is `(descriptor_id, previous_generation)`. Existing live PTYs are returned/referenced, never killed/recreated.

## Fixed semantics / legacy

HookKind remains the existing ten values (native turn_start/agent_start → prompt_submitted; turn_end/agent_end → turn_done). Runtime state is busy/idle/needs_approval/blocked/error/ended. RestoreState is separate: pending_confirmation/confirmed/unconfirmed/stale/interrupted/ended/snapshot_missing. `legacy-unconfirmed` is an identity source, not confirmation or permission for conversation auto-restore.

Default native sets: Claude SessionStart/UserPromptSubmit/Stop/SessionEnd plus version-gated Notification/PermissionRequest; pi session_start/agent_start/agent_end/session_shutdown; qin session_start/turn_start/turn_end/session_end. Tool events default off. Notification cases: permission_prompt→needs_approval, idle_prompt→idle; unknown fails closed. pi legacy turn_start/turn_end/session_end/tool_execution_* aliases stay snapshot-bound; v1/v2 recognition is byte-exact, not header/alias inference. Resume variants: claude `--resume session_id`, pi `--session transcript_path`, qin-code/self-dev `--resume session_id`; variant comes from host evidence, never renderer choice.

Legacy golden bytes live in `src-tauri/src/agent_hooks/tests/fixtures/legacy/`. A must retain the renderer reproduction test for legacy recognition; v3 bytes are intentionally different. Identity excludes runtime revision, hook_at, kind and tools; only identity changes may affect layout persistence.
