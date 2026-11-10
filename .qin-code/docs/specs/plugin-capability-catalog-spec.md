---
schema: qin-doc/1
kind: spec
id: plugin-capability-catalog-spec
title: 插件可调用的 Se Manager 全量能力清单
date: 2026-11-04
status: accepted
source: user:2026-11-04T16:59:29+08:00 只有凭据和秘钥不开放，其它都开放给插件; agent://cat-a; agent://cat-b; agent://cat-c; user:2026-11-07 req-a1fec49128c8a770f1a9b055e56197e7 MCP host workspace/SSH catalog sync; agent://catalog-host-evidence; src-tauri/src/commands.rs; src-tauri/src/lib.rs; src-tauri/src/web/mcp_host_api.rs; src-tauri/src/mcp_core/host_bridge.rs; src-tauri/src/credentials.rs; user:2026-11-07 req-8f01105f55c188c39fdf0e9609e35343 SSH command deny v2/management preview catalog sync; .qin-code/docs/proposals/ssh-command-deny-policy-proposal.md; src-tauri/src/web/router.rs; src/shared/types/mcp-host.types.ts
updated: 2026-11-10
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_lizard_1794024668506_6ffbe76ff784f5bf","last_updated_by_session":"session_clover_1794039875986_a9f773ef52155584","session_ids":["session_clover_1794039875986_a9f773ef52155584","session_lizard_1794024668506_6ffbe76ff784f5bf"],"todo_refs":[],"goal_refs":[]}
---

## 契约

### 开放政策（用户决定，2026-11-04）

除凭据和密钥外，Se Manager 的全部能力都开放给插件，包括项目、终端、Agent 会话、Git、SSH/SFTP、MCP、定时任务、浏览器、主题和布局等。规则如下：

1. **只有读写密钥值本身的操作不开放（SECRET-EXCLUDED）。** 这里的密钥值指：API key、SSH 密码与私钥、askpass、OAuth token、安全存储里的值、终端 claim 凭据，以及 webview localStorage 交接数据。
2. **使用凭据但不返回凭据的操作照常开放。** 例如用已保存配置连接 SSH、`git push`、测试 AI 渠道、完成 MCP OAuth。
3. **原本会带出密钥的操作，去掉密钥后开放（OPEN-REDACTED）。** 返回值把 token、env、header 换成 `configured`、`authenticated`、`hasToken` 之类的布尔值；写入接口的回显里也不带密钥。
4. **危险操作同样开放，但按 `confirm` 列确认：**
   - `per-call`：每次调用都要用户在本机确认，用于删除、强推、向终端写原始输入、批准 agent 工具调用、安装原生代码等；
   - `grant`：插件安装时授权一次即可；
   - `none`：无需确认。
5. **纯界面内部通信的命令（INTERNAL）不单独登记。** 它们由清单里注明的上层 op 覆盖。

### 统计

本表按 `src-tauri/src/lib.rs` 的 `generate_handler!` 逐条核对（2026-11-07 宿主 MCP、SSH 命令黑名单及独立记忆入口退役同步后用脚本比对）：后端共 347 个命令，全部已分类。合并为同一 op 的命令按各自原有分类计入，重复登记的只计一次。

| 来源 | 总数 | OPEN | OPEN-REDACTED | SECRET-EXCLUDED | INTERNAL |
|---|---|---|---|---|---|
| 后端 Tauri 命令 | 347 | 309 | 12 | 9 | 17 |
| 前端独有能力（无 Rust 命令） | 39 | 39 | 0 | 0 | 0 |
| **合计** | **386** | **348** | **12** | **9** | **17** |

各部分的范围：
- A：Agent、会话、对话、记忆、Skills、定时任务、AI 渠道、MCP、远程访问、插件；
- B：终端、快捷终端、Git、worktree、搜索、文件、工作区、SSH/SFTP、隧道；
- C：浏览器、数据迁移、安全存储、日志、系统，以及前端独有能力。

各部分内部的计数表沿用 explorer 的原始统计，与本表不一致时以本表为准。

### 事件订阅

插件通过统一订阅入口接收事件：
- A：30 个，包括 `acp:*`、对话生命周期、记忆索引进度、Skills 进度、插件状态、OAuth 完成；
- B：20 个，包括终端输出/退出/cwd/git 状态、搜索流、文件监听、SSH 与 SFTP 进度、clone 进度；
- C：前端事件 6 类，即 `project.changed`、`project.active.changed`、`settings.changed`、`theme.changed`、`shortcuts.changed`、`layout.changed`。

事件只携带 revision 和 id，不携带文档内容；插件拿到事件后，要在自己的授权范围内再读取内容。

### 提供方

op 按实际执行位置分为三类：
- **rust-main（主进程）：** 后端 Tauri 命令对应的 op；
- **renderer（前端）：** 项目增删改与分组、应用设置、主题、快捷键、布局和工作区标签，这些只存在于前端 store；
- **mcp-core / plugin-core：** 记忆 MCP 等现有服务。

所有 op 都经 `plugin-platform-plan` 的 MCP runtime 统一注册表登记，再投影为 MCP 工具、插件宿主 API 和命令面板入口。

### 清单 A：Agent、会话、对话、记忆、Skills、定时任务、AI 渠道、MCP、远程访问、插件

### Se Manager — Plugin Capability Catalog, Part A

Repo: `/Users/qs/project/me/termul` (read-only; nothing edited). All paths below are relative to `src-tauri/src/`.

**Policy applied:** every capability is OPEN except reading/writing secret values. Ops that would return secrets are registered as OPEN-REDACTED (they return `configured` / `authenticated` booleans), and only the single op whose purpose *is* a secret is SECRET-EXCLUDED.

**Coverage:** 156 in-scope commands → 152 registry rows. Five `acp_set_*_timeout` commands collapse into one row (`se.agents.setRuntimeTimeout`); every other command has its own row, so the table itself is the command↔op bijection.

Conventions: `confirm=none` (read or harmless), `grant` (install-time grant is enough), `per-call` (destructive/irreversible or network-install/credential-rotation). Long-running is stated per row.

---

#### 1. Plugin runtime — `plugin_commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.plugins.uiOpen | plugin_web_open `plugin_commands.rs:231` | W | pluginSelf | grant | OPEN | in `{sessionId, pluginId, entry, size}`; out `{sessionId, url}`; opens a plugin webview on a per-session scheme origin; requires the `main` window |
| se.plugins.uiClose | plugin_web_close `plugin_commands.rs:257` | W | pluginSelf | none | OPEN | in `{sessionId}`; out `WebCloseReply`; also drops the `PluginUiSessions` entry; main-window only |
| se.plugins.webRead | plugin_web_read `plugin_commands.rs:302` | R | pluginSelf | none | OPEN | in `{sessionId, path}`; out `WebReadReply` bytes; path is served from the plugin bundle (confinement unverified — see open issues) |
| se.plugins.webRequest | plugin_web_request `plugin_commands.rs:308` | W | pluginSelf | grant | OPEN | in `{sessionId, method, url, headers, body}`; out raw JSON; plugin-supplied headers/body may carry its own secrets; SSRF/egress surface |
| se.plugins.processList | plugin_process_list `plugin_commands.rs:315` | R | pluginSelf | none | OPEN | in `{}`; out `ProcessListReply` (pid, plugin, state) |
| se.plugins.processLogs | plugin_process_logs `plugin_commands.rs:321` | R | pluginSelf | none | OPEN | in `{processId, lines}`; out `HostProcessLogsReply`; log text can echo plugin env |
| se.plugins.processControl | plugin_process_control `plugin_commands.rs:327` | D | pluginSelf | per-call | OPEN | in `{processId, action}`; stops/starts a plugin host process |
| se.plugins.list | plugin_list `plugin_commands.rs:33` | R | global | none | OPEN | out `PluginSnapshot` (plugins, enabled, Core connection) |
| se.plugins.install | plugin_install `plugin_commands.rs:51` | W | global | per-call | OPEN | in `InstallRequest{source}`; loads third-party native code; network + disk |
| se.plugins.setEnabled | plugin_set_enabled `plugin_commands.rs:52` | W | global | grant | OPEN | in `{pluginId, enabled}`; out `PluginInfo` |
| se.plugins.reload | plugin_reload `plugin_commands.rs:58` | W | global | grant | OPEN | in `{pluginId}`; restarts the plugin, out `PluginInfo` |
| se.plugins.uninstall | plugin_uninstall `plugin_commands.rs:59` | D | global | per-call | OPEN | in `{pluginId}`; out `UninstallResult`; deletes installed files |
| se.plugins.getView | plugin_get_view `plugin_commands.rs:65` | R | pluginSelf | none | OPEN | in `{pluginId, viewId}`; out `ViewSnapshot` |
| se.plugins.dispatchAction | plugin_dispatch_action `plugin_commands.rs:71` | W | pluginSelf | grant | OPEN | in `ActionEvent{pluginId, actionId, payload}`; out `ViewSnapshot` |
| se.plugins.executeCommand | plugin_execute_command `plugin_commands.rs:77` | W | pluginSelf | per-call | OPEN | in `{pluginId, command, args}`; out `CommandReply`; can mutate host state |
| se.plugins.getConfig | plugin_get_config `plugin_commands.rs:83` | R | pluginSelf | none | OPEN | in `{pluginId}`; out `ConfigSnapshot`; redact any field the manifest marks secret |
| se.plugins.setConfig | plugin_set_config `plugin_commands.rs:89` | W | pluginSelf | grant | OPEN | in `{pluginId, values}`; input may hold plugin secrets — never echo |
| se.plugins.publishHostContext | plugin_publish_host_context `plugin_commands.rs:158` | W | pluginSelf | grant | OPEN | in parsed host context (project/session/selection); out `HostContextAck`; main-window only |
| se.plugins.heartbeatHostContext | plugin_heartbeat_host_context `plugin_commands.rs:168` | W | pluginSelf | none | OPEN | in `{sourceId, …}`; lease renewal, out `HostContextAck`; main-window only |
| se.plugins.getHostUi | plugin_get_host_ui `plugin_commands.rs:178` | R | pluginSelf | none | OPEN | in parsed get request; out `HostUiSnapshot` |
| se.plugins.dismissHostUi | plugin_dismiss_host_ui `plugin_commands.rs:209` | W | pluginSelf | none | OPEN | in parsed dismiss request; out `HostUiDismissResult` |
| se.plugins.executeContribution | plugin_execute_contribution `plugin_commands.rs:215` | W | pluginSelf | grant | OPEN | in execute-contribution request (menu/command contribution); out `CommandReply` |

---

#### 2. ACP agents & sessions — `acp/commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.agents.spawn | acp_spawn_agent `acp/commands.rs:196` | W | profile | grant | OPEN | in runtime/agent id + cwd; out `agentId`; **long-running** (spawns the ACP child) |
| se.agents.kill | acp_kill_agent `acp/commands.rs:220` | D | session | per-call | OPEN | in `agentId`; kills runtime and its live sessions |
| se.agents.list | acp_list_agents `acp/commands.rs:235` | R | global | none | OPEN | out agents[] with runtime/status |
| se.agents.setPermissionPolicy | acp_set_permission_policy `acp/commands.rs:248` | W | profile | grant | OPEN | in policy (allow/deny/ask scope); security-relevant and host-wide |
| se.sessions.create | acp_new_session `acp/commands.rs:276` | W | project | grant | OPEN | in agentId + cwd; out sessionId + config options; **long-running** (handshake) |
| se.sessions.load | acp_load_session `acp/commands.rs:350` | W | session | grant | OPEN | in sessionId; hydrates a persisted session; **long-running** |
| se.sessions.resume | acp_resume_session `acp/commands.rs:403` | W | session | grant | OPEN | in sessionId; same shape as load — merge as one op with `mode=load|resume` |
| se.sessions.close | acp_close_session `acp/commands.rs:455` | D | session | grant | OPEN | in sessionId; ends the live session but keeps history |
| se.sessions.disposeEphemeral | acp_dispose_ephemeral_session `acp/commands.rs:504` | D | session | per-call | OPEN | in sessionId; discards an ephemeral session and its records |
| se.sessions.list | acp_list_sessions `acp/commands.rs:547` | R | project | none | OPEN | out live sessions[] |
| se.sessions.registerDiscovered | acp_register_discovered_session `acp/commands.rs:569` | W | project | grant | OPEN | in CLI-discovered metadata (title, cwd, agent); imports into the host index |
| se.sessions.sendPrompt | acp_send_prompt `acp/commands.rs:640` | W | session | grant | OPEN | in sessionId + prompt blocks; streams `acp:message_chunk` / `acp:tool_call`; **long-running** |
| se.sessions.cancelPrompt | acp_cancel_prompt `acp/commands.rs:767` | W | session | none | OPEN | in sessionId; cancels the in-flight turn |
| se.sessions.setConfigOption | acp_set_config_option `acp/commands.rs:788` | W | session | grant | OPEN | in sessionId, configId, value |
| se.sessions.setMode | acp_set_mode `acp/commands.rs:818` | W | session | grant | OPEN | in sessionId + modeId |
| se.sessions.setModel | acp_set_model `acp/commands.rs:844` | W | session | grant | OPEN | in sessionId + modelId |
| se.agents.authenticate | acp_authenticate `acp/commands.rs:871` | W | profile | grant | OPEN | in agentId (+methodId); uses stored/driver credentials, returns auth status only |
| se.sessions.respondPermission | acp_respond_permission `acp/commands.rs:904` | W | session | per-call | OPEN | in sessionId, requestId, optionId; approves a tool call — must never be grant-scoped |
| se.sessions.answerQuestion | acp_answer_question `acp/commands.rs:952` | W | session | none | OPEN | in sessionId, questionId, answers |
| se.agents.probeRuntime | acp_probe_runtime `acp/commands.rs:992` | R | global | none | OPEN | out `AcpRuntimeProbe` (installed/version per runtime) |
| se.agents.listCatalog | acp_list_catalog `acp/commands.rs:1004` | R | global | none | OPEN | out catalog entries from the cached registry |
| se.agents.setCatalogOptIn | acp_set_catalog_opt_in `acp/commands.rs:1062` | W | profile | grant | OPEN | in enabled flag |
| se.agents.install | acp_install_agent `acp/commands.rs:1107` | W | profile | per-call | OPEN | in agentId/version; verified-atomic download + install; network + disk |
| se.agents.probeMcpServer | acp_probe_mcp_server `acp/commands.rs:1170` | W | profile | per-call | OPEN | in server config; spawns a probe connection and lists tools; env secrets in input are used, never returned |
| se.agents.fetchRegistrySnapshot | acp_fetch_registry_snapshot `lib.rs:3971` (impl in `acp_registry_snapshot`) | R | profile | none | OPEN | network fetch of the ACP registry snapshot, cached on disk; definition line not opened |
| se.agents.installRegistryBinary | acp_install_registry_binary `lib.rs:3972` (impl in `acp_binary_install`) | W | profile | per-call | OPEN | downloads + installs a registry binary; network + disk; definition line not opened |
| se.agents.setRuntimeTimeout | acp_set_turn_timeout `:1184`, acp_set_turn_idle_timeout `:1195`, acp_set_session_new_timeout `:1209`, acp_set_session_reopen_timeout `:1224`, acp_set_first_prompt_warmup_timeout `:1239` | W | profile | grant | OPEN | merged: five commands → one op; in `{kind, seconds|null}`; sets process-global ACP timers |
| se.agents.setPreferLocalNpmInstall | acp_set_prefer_local_npm_install `acp/commands.rs:1249` | W | profile | grant | OPEN | in bool; changes install strategy |

---

#### 3. Desktop ACP history — `commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.sessions.historyList | acp_history_list `commands.rs:6381` | R | project | none | OPEN | out `{sessions[], legacyImportComplete}`; proxies `historyList` when an ACP Core client exists |
| se.sessions.historyGet | acp_history_get `commands.rs:6563` | R | session | none | OPEN | in sessionId; out full payload; rejects >4 MiB or too many records (`CONVERSATION_HISTORY_PAGING_REQUIRED`) |
| se.sessions.historyGetPage | acp_history_get_page `commands.rs:6642` | R | session | none | OPEN | in sessionId, afterSeq, limit, targetLastSeq; out page + nextCursor; prefer this over historyGet (merge as `mode=page`) |
| se.conversations.historySaveLegacy | acp_history_save `commands.rs:6675` | W | session | none | INTERNAL | always rejects `LEGACY_STORE_READ_ONLY`; capability is se.sessions.sendPrompt |
| se.conversations.historyDeleteLegacy | acp_history_delete `commands.rs:6691` | W | session | none | INTERNAL | always rejects `LEGACY_STORE_READ_ONLY` |
| se.conversations.historyFlushLegacy | acp_history_flush `commands.rs:6705` | W | session | none | INTERNAL | no-op success ack ("appends are already durable") |
| se.conversations.historyMarkLegacy | acp_history_mark_legacy_import_complete `commands.rs:6714` | W | global | none | INTERNAL | always rejects `LEGACY_STORE_READ_ONLY` |
| se.conversations.historyListLegacy | acp_history_list_legacy `commands.rs:6732` | R | project | none | INTERNAL | reads the legacy store only for the one-time KV wipe migration |
| se.conversations.historyGetLegacy | acp_history_get_legacy `commands.rs:6744` | R | session | none | INTERNAL | legacy payload read for the same migration |

---

#### 4. Conversations — `commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.conversations.hostStatus | conversation_host_status `commands.rs:7285` | R | global | none | OPEN | out `ConversationHostStatus` (admission + migration state) |
| se.conversations.list | conversation_list `commands.rs:7309` | R | project | none | OPEN | out `ConversationRecordV2[]` |
| se.conversations.get | conversation_get `commands.rs:7336` | R | session | none | OPEN | in conversationId |
| se.conversations.getBinding | conversation_get_binding `commands.rs:7370` | R | session | none | OPEN | out `ConversationBindingSnapshot` |
| se.conversations.rename | conversation_rename `commands.rs:7390` | W | session | none | OPEN | in conversationId, title; out record |
| se.conversations.open | conversation_open `commands.rs:7438` | W | session | grant | OPEN | in conversationId; may spawn/resume an agent; **long-running** |
| se.conversations.resolveLegacyId | conversation_resolve_legacy_id `commands.rs:7479` | R | project | none | OPEN | in `LegacyConversationKey` payload; out resolution |
| se.conversations.recoveryResolve | conversation_recovery_resolve `commands.rs:7588` | W | project | per-call | OPEN | in recovery request; emits `conversation:host-status`; migration decisions are hard to undo |
| se.conversations.attachProject | conversation_attach_project `commands.rs:7644` | W | session | none | OPEN | in conversationId, expectedRevision, attachment; emits `conversation:aggregate` |
| se.conversations.detachProject | conversation_detach_project `commands.rs:7700` | W | session | none | OPEN | in conversationId, expectedRevision |
| se.conversations.setExecutionTarget | conversation_update_execution_target `commands.rs:7764` | W | session | grant | OPEN | in conversationId, expectedRevision, executionTarget (local/worktree/ssh) |
| se.conversations.detachBinding | conversation_detach_binding `commands.rs:7817` | W | session | per-call | OPEN | in conversationId, expectedRevision; unbinds the live chat |
| se.conversations.rebindBinding | conversation_rebind_detached_binding `commands.rs:7853` | W | session | grant | OPEN | in conversationId, expectedRevision |
| se.conversations.suspendBinding | conversation_suspend_binding `commands.rs:7889` | W | session | per-call | OPEN | in conversationId, expectedRevision |
| se.conversations.replaceBinding | conversation_replace_binding `commands.rs:7925` | D | session | per-call | OPEN | in conversationId, expectedRevision, request, targetRuntimeAgentId (None = same agent); discards the current binding |
| se.conversations.delete | conversation_delete `commands.rs:8033` | D | session | per-call | OPEN | in conversationId (+force); deletes records, emits `conversation:lifecycle` |
| se.conversations.migrationControl | conversation_migration_control `commands.rs:274` | W | global | per-call | OPEN | sync `pub fn`; schedules restart-required migration maintenance; argument shape not read |

---

#### 5. Per-conversation workspace

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.sessions.getWorkspace | session_workspace_get `commands.rs:7494` | R | session | none | OPEN | in conversationId; out `SessionWorkspaceLoadOutcome` |
| se.sessions.writeWorkspace | session_workspace_write `commands.rs:7519` | W | session | none | OPEN | in conversationId, basedRevision?, workspace; optimistic concurrency, out write outcome |

---

#### 6. Memory index — `memory_index/commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.memory.buildIndex | memory_index_build_cmd `memory_index/commands.rs:142` | W | project | grant | OPEN | in `{projectRoot, fullRebuild, indexUnscoped}`; out `IngestReport`; **long-running** (154 s measured), emits `acp:memory_index_progress` throttled to 150 ms |
| se.memory.indexStatus | memory_index_status_cmd `memory_index/commands.rs:204` | R | project | none | OPEN | in `{projectRoot}`; out `MemoryIndexStatus` |
| se.memory.search | memory_index_search_cmd `memory_index/commands.rs:228` | R | project | none | OPEN | in `{projectRoot, query, filters}`; out `MemorySearchResponse` |
| se.memory.listSessions | memory_index_sessions_cmd `memory_index/commands.rs:252` | R | project | none | OPEN | in `{projectRoot, limit, includeUnscoped, agents[]}` |
| se.memory.getSession | memory_index_session_cmd `memory_index/commands.rs:281` | R | project | none | OPEN | in `{projectRoot, sessionKey, limit, includeStale, includeUnscoped}` |

---

#### 7. Skills — `skills/commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.skills.status | skills_status_cmd `skills/commands.rs:15` | R | project | none | OPEN | in `{projectId?, projectRoot?}`; out `SkillsStatus` |
| se.skills.refresh | skills_refresh_cmd `skills/commands.rs:34` | W | project | grant | OPEN | in `{projectId?, projectRoot?}`; rescans and syncs installed skills |
| se.skills.sync | skills_sync_cmd `skills/commands.rs:53` | W | project | grant | OPEN | exact alias — delegates to `skills_refresh_cmd`; merge, keep the name as an alias |
| se.skills.preview | skills_preview_cmd `skills/commands.rs:62` | W | project | grant | OPEN | in `SkillsPreviewRequest`; out `{jobId}`; emits `skills_operation_progress` every 100 ms |
| se.skills.operationStatus | skills_operation_status_cmd `skills/commands.rs:75` | R | project | none | OPEN | in jobId; out `SkillsOperationStatus` |
| se.skills.cancelOperation | skills_cancel_operation_cmd `skills/commands.rs:85` | W | project | none | OPEN | in jobId |
| se.skills.installPreview | skills_install_preview_cmd `skills/commands.rs:95` | W | project | grant | OPEN | in `SkillsInstallCommit`; downloads and stages, out `{jobId}` |
| se.skills.install | skills_install_cmd `skills/commands.rs:127` | W | project | per-call | OPEN | in `SkillsInstallRequest`; network + writes skill files; out `SkillManifest` |
| se.skills.project | skills_project_cmd `skills/commands.rs:138` | W | project | grant | OPEN | in `SkillsProjectionRequest`; writes into an agent's skills dir |
| se.skills.repair | skills_repair_cmd `skills/commands.rs:153` | W | project | grant | OPEN | in `SkillsRepairRequest`; rewrites broken installs |
| se.skills.listAgentSkills | list_agent_skills_cmd `skills/commands.rs:164` | R | project | none | OPEN | in projectRoot?; out `AgentSkillSummary[]` |
| se.skills.readAgentSkill | read_agent_skill_cmd `skills/commands.rs:171` | R | project | none | OPEN | in name, projectRoot?; out `AgentSkillContent` (SKILL.md) — user files, not credentials |

---

#### 8. Scheduled tasks — `scheduled_tasks/commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.scheduled.preview | scheduled_task_preview `scheduled_tasks/commands.rs:50` | R | project | none | OPEN | in schedule, count?; out `SchedulePreviewV1` (next N fire times) |
| se.scheduled.list | scheduled_task_list `scheduled_tasks/commands.rs:71` | R | project | none | OPEN | in projectId?; out tasks[] |
| se.scheduled.get | scheduled_task_get `scheduled_tasks/commands.rs:91` | R | project | none | OPEN | in taskId |
| se.scheduled.createDraft | scheduled_task_draft_create `scheduled_tasks/commands.rs:111` | W | project | grant | OPEN | in `ScheduledTaskDraftInputV1`; out draft task |
| se.scheduled.updateDraft | scheduled_task_draft_update `scheduled_tasks/commands.rs:131` | W | project | grant | OPEN | in taskId, expectedRevision, input |
| se.scheduled.activate | scheduled_task_activate `scheduled_tasks/commands.rs:157` | W | project | per-call | OPEN | in taskId, expectedRevision, expectedDraftHash; starts unattended agent runs |
| se.scheduled.pause | scheduled_task_pause `scheduled_tasks/commands.rs:183` | W | project | none | OPEN | in taskId, expectedRevision |
| se.scheduled.resume | scheduled_task_resume `scheduled_tasks/commands.rs:208` | W | project | grant | OPEN | in taskId, expectedRevision |
| se.scheduled.delete | scheduled_task_delete `scheduled_tasks/commands.rs:233` | D | project | per-call | OPEN | in taskId, expectedRevision; drops task + run history |
| se.scheduled.runNow | scheduled_task_run_now `scheduled_tasks/commands.rs:258` | W | project | per-call | OPEN | in taskId; spawns an agent turn immediately; **long-running** |
| se.scheduled.retryRun | scheduled_task_retry_run `scheduled_tasks/commands.rs:278` | W | project | per-call | OPEN | in taskId, runId |
| se.scheduled.listRuns | scheduled_task_list_runs `scheduled_tasks/commands.rs:299` | R | project | none | OPEN | in taskId; out runs[] (may reference transcript paths) |
| se.scheduled.listAudit | scheduled_task_list_audit `scheduled_tasks/commands.rs:319` | R | project | none | OPEN | in taskId; out audit events[] |

---

#### 9. AI channels — `commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.ai.listChannels | ai_channels_get `commands.rs:5824` | R | profile | none | OPEN | out `{document, keys: id→bool}`; key values never leave the host credential backend (user-private file, not OS keychain) |
| se.ai.saveChannels | ai_channels_put `commands.rs:5848` | W | profile | grant | OPEN | in `AiChannelsDocument` (endpoint/model only; no secret fields) |
| se.ai.setChannelKey | ai_channel_set_key `commands.rs:5862` | SEC | profile | per-call | SECRET-EXCLUDED | stores or (`key: null`) deletes the channel API key in the host credential backend (user-private file, not OS keychain); this op's purpose is the secret itself (a write-only re-open is a policy call) |
| se.ai.listModels | ai_channel_models `commands.rs:5889` | R | profile | none | OPEN | in channel config; uses the stored key; out model ids |
| se.ai.testChannel | ai_channel_test `commands.rs:5911` | R | profile | none | OPEN | in channel + model; one 64-token completion using the stored key; out `{reply≤200 chars, millis}` |

---

#### 10. MCP control plane — `commands.rs`

双来源运行边界：`internal`（Se内置）与`aggregator`（外部上游）各自持有独立core、端口、settings与token，不存在Mixed。三种mode仅呈现本来源。新`GET /mcp-core/identity`在每个纯来源端口返回固定protocol/protocolVersion/source，无令牌或路径，作为`se.mcp.getRuntimeStatus`覆盖的公开只读协议元数据，不新增Tauri计数。所有业务及控制请求仍需对应bearer，带错source/version marker时在分发前拒绝。bridge同origin且先无凭据握手，旧mixed无marker不得接入；日志不输出token、settings或原始URL/响应体。HTTP/stdio导出和本机生命周期管理继续desktop-only；standalone内部双来源listener与ACP注入同样隔离，不新增Web凭据导出。描述与OAuth针对聚合器；全局控制文档保持单份并按来源投影。

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.mcp.getConfig | mcp_get_config `commands.rs:5647` | R | profile | none | OPEN-REDACTED | out the control-plane document; strip `env` / `headers` values, keep key names |
| se.mcp.putConfig | mcp_put_config `commands.rs:5657` | W | profile | per-call | OPEN-REDACTED | in full document (may carry env/header secrets); writes + reloads both source projections; never echo secret fields |
| se.mcp.getStatus | mcp_get_status `commands.rs:5670` | R | profile | none | OPEN | out redacted `McpControlPlaneStatus` |
| se.mcp.getRuntimeStatus | mcp_get_runtime_status `commands.rs:5679` | R | profile | none | OPEN | in source? (`internal` default / `aggregator`); out source-bound `ServiceView` (running, port, source, identity, own upstream states) |
| se.mcp.startGateway | mcp_service_start `commands.rs:5687` | W | profile | grant | OPEN | in source?; starts/upgrades only the selected pure-source gateway; missing/mismatched identity never enables mixed forwarding |
| se.mcp.restartGateway | mcp_service_restart `commands.rs:5697` | W | profile | grant | OPEN | in source?; restarts only that source, preserving the other service |
| se.mcp.setGatewayPort | mcp_service_set_port `commands.rs:5709` | W | profile | grant | OPEN | in source?, port; checks cross-source port conflict, rewrites only selected settings with rollback on failure |
| se.mcp.getConnectionInfo | mcp_connection_info `commands.rs:5731` | R | profile | none | OPEN-REDACTED | in source?, mode?; out `{source, mode, bridge, url, httpReady, ...}` **contains bearer token** — strip token and auth headers, return `hasToken`; `se-mcp` is internal-only and `se-mcp-aggregator` upstream-only; stdio export uses bridge only, HTTP copy requires verified identity; no authorization changes or se-memory revival |
| se.mcp.detectClients | mcp_clients_detect `commands.rs:5747` | R | profile | none | OPEN | in source?, mode?; out `DetectedClient[]` for the selected fixed client key; old args require explicit update |
| se.mcp.syncClient | mcp_client_sync `commands.rs:5768` | W | profile | per-call | OPEN | in clientId, source?, mode?; writes only `se-mcp` or `se-mcp-aggregator`, preserving the companion entry and third-party config under a common mutation lock |
| se.mcp.unsyncClient | mcp_client_unsync `commands.rs:5793` | W | profile | per-call | OPEN | in clientId, source?; removes only the selected fixed key, preserving the companion entry |
| se.mcp.getDescriptions | mcp_descriptions_get `commands.rs:5943` | R | profile | none | OPEN | out map configId→description |
| se.mcp.setDescriptions | mcp_descriptions_put `commands.rs:5952` | W | profile | grant | OPEN | in map; reloads aggregator only |
| se.mcp.getDescribeSettings | mcp_describe_settings_get `commands.rs:5976` | R | profile | none | OPEN | out `{settings, defaults}` |
| se.mcp.setDescribeSettings | mcp_describe_settings_put `commands.rs:5991` | W | profile | grant | OPEN | in `DescribeSettings` |
| se.mcp.describeServers | mcp_describe_servers `commands.rs:6020` | W | profile | per-call | OPEN | in names?, language; calls the `mcp_summary` AI channel with the stored key, stores text, reloads; **long-running** (LLM, concurrency 4) |
| se.mcp.listServers | mcp_service_servers `commands.rs:6129` | R | profile | none | OPEN | out selected source server list + descriptions; in source? default internal; never a runtime union |
| se.mcp.listServerTools | mcp_service_server_tools `commands.rs:6140` | R | profile | none | OPEN | in source?, name; out tool schemas from that source only |
| se.mcp.beginOauth | begin_mcp_oauth `commands.rs:5502` | W | profile | per-call | OPEN | in id, challenge?; out `{authorizationUrl, expiresAt}`; opens a browser + loopback callback listener |
| se.mcp.completeOauth | complete_mcp_oauth `commands.rs:5600` | W | profile | per-call | OPEN | in id, code, state; exchanges the code into the host credential backend (user-private file, not OS keychain); out redacted status |
| se.mcp.cancelOauth | cancel_mcp_oauth `commands.rs:5618` | W | profile | none | OPEN | in id; drops the pending flow, keeps any prior grant |
| se.mcp.getOauthStatus | get_mcp_oauth_status `commands.rs:5628` | R | profile | none | OPEN | in id; out `{state, hasCredentials, canRefresh, expiresAt, authorizationRequired}` — booleans only |

---

#### 11. Remote access — `commands.rs`

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.remote.startServer | remote_server_start `commands.rs:4658` | W | profile | per-call | OPEN | in bindMode? (`localhost`|`all`); starts the shared-live web server + tunnel; persists intent; LAN mode exposes all interfaces |
| se.remote.stopServer | remote_server_stop `commands.rs:4843` | W | profile | grant | OPEN | stops server, clears the in-memory project mirror; live agents survive |
| se.remote.getStatus | remote_server_status `commands.rs:4875` | R | profile | none | OPEN | out `RemoteStatus`; verify it carries no pairing bearer before exposing |
| se.remote.getAccessIntent | remote_access_intent_get `commands.rs:4896` | R | profile | none | OPEN | out `{wanted, publishMode}` |
| se.remote.setAccessIntent | remote_access_intent_set `commands.rs:4909` | W | profile | grant | OPEN | in `{wanted?, publishMode?}` (lan|tunnel) |
| se.remote.rotateCredential | remote_server_rotate_credential `commands.rs:4941` | W | profile | per-call | OPEN-REDACTED | rotates the remote bearer; out `RemoteStatus` — strip the new token |
| se.remote.devices.list | remote_devices_list `web/devices_api.rs` | R | profile | none | OPEN | local operator/manage only; host-wide device metadata and connection counts, never credential digests |
| se.remote.devices.rename | remote_device_rename `web/devices_api.rs` | W | profile | grant | OPEN | local operator/manage only; in id,name |
| se.remote.devices.setScopes | remote_device_set_permissions `web/devices_api.rs` | W | profile | per-call | OPEN | local operator/manage only; eight grantable scopes; remote devices can never receive manage |
| se.remote.devices.grantAll | remote_device_grant_all `web/devices_api.rs` | W | profile | per-call | OPEN | local operator/manage only; grants eight scopes, excludes manage |
| se.remote.devices.disconnect | remote_device_disconnect `web/devices_api.rs` | W | profile | grant | OPEN | local operator/manage only; closes this device's connections without invalidating credential |
| se.remote.devices.revoke | remote_device_revoke `web/devices_api.rs` | W | profile | per-call | OPEN | local operator/manage only; durable credential revocation and WS policy close 1008; re-pair required |
| se.remote.devices.disconnectAll | remote_devices_disconnect_all `web/devices_api.rs` | W | profile | per-call | OPEN | local operator/manage only; force revokes all paired devices and legacy host bearer |
| se.remote.devices.audit | remote_devices_audit `web/devices_api.rs` | R | profile | none | OPEN | local operator/manage only; durable boundary events contain device ids, never tokens |
| se.remote.pairingCode | remote_pairing_code `web/devices_api.rs` | S | profile | — | SECRET-EXCLUDED | one-time pairing credential, TTL60s; desktop operator only; never expose through plugins |
| se.remote.syncProjects | remote_sync_projects `commands.rs:5030` | W | project | grant | OPEN | in `{projects[], groups[], defaultProjectId}`; broadcasts `acp:projects_changed`; project summaries redact env by omission |
| se.remote.syncChatHistory | remote_sync_chat_history `commands.rs:5128` | W | project | grant | OPEN | pushes desktop chat history into the project file; **long-running** |
| se.remote.syncMcpRegistry | remote_sync_mcp_registry `commands.rs:5157` | W | project | grant | OPEN-REDACTED | in registry document (may hold env/header secrets) → `mcp-servers.json`; never echo |
| se.remote.loadMcpRegistry | remote_load_mcp_registry `commands.rs:5185` | R | project | none | OPEN-REDACTED | out raw registry document; strip env/header values |

---

#### 12. Discovery & host defaults

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.agents.fetchRegistry | agent_registry_fetch `commands.rs:1940` | R | profile | none | OPEN | in forceRefresh?; network fetch, disk cache, degrades to cache/empty; `distribution` deliberately omitted |
| se.host.setDefaultProject | set_host_default_project `commands.rs:5064` | W | global | grant | OPEN | in projectId; validates switchable, broadcasts `projects_changed` |
| se.sessions.discoverCli | list_cli_sessions_cmd `cli_session/commands.rs:9` | R | project | none | OPEN | in `{scopePaths?}`; scans local agent session files (titles/paths — may include prompt text) |
| se.sessions.resolveCli | resolve_cli_sessions_cmd `cli_session/commands.rs:24` | R | project | none | OPEN | in `{files[]}`; reads those transcripts |
| se.terminals.detectLiveAgents | detect_live_agent_sessions_cmd `cli_session/commands.rs:40`; GET /cli-sessions/live (`web/cli_session_api.rs`) | R | terminal | none | OPEN | in `rootPids[]`; reads this machine's process table. Web route derives roots from live terminal-service PTYs and returns `{sessions:[{terminalId,projectId?,rootPid,agentId,sessionId,selfDev?,codexHome?,cwd?}],scannedAt}` — read-only, no secrets |

| se.agentHooks.status | agent_hooks_status_cmd `agent_hooks/commands.rs`; GET /agent-hooks/status (`web/agent_hooks_api.rs`) | R | global | none | OPEN-REDACTED | Cached adapter status; never probes or installs. No endpoint/bearer values. |
| se.agentHooks.reload | agent_hooks_reload_cmd `agent_hooks/commands.rs`; POST /agent-hooks/reload (`web/agent_hooks_api.rs`) | R | global | none | OPEN-REDACTED | Explicit resource refresh only, no CLI spawn or installation; manage-only on HTTP. |
| se.agentHooks.probe | agent_hooks_probe_cmd `agent_hooks/commands.rs`; POST /agent-hooks/probe (`web/agent_hooks_api.rs`) | R | global | none | OPEN-REDACTED | Explicit bounded CLI version probe, validated adapter_id; manage-only on HTTP. |
| se.agentHooks.preview | agent_hooks_preview_cmd `agent_hooks/commands.rs`; POST /agent-hooks/preview (`web/agent_hooks_api.rs`) | R | global | none | OPEN-REDACTED | Full CAS diff for adapter/action/event_options; credentials redacted, manage-only; no plugin grant protocol in v3. |
| se.agentHooks.confirm | agent_hooks_confirm_cmd `agent_hooks/commands.rs`; POST /agent-hooks/confirm (`web/agent_hooks_api.rs`) | W | global | per-call | OPEN | Manage-only CAS confirm bound to preview/fingerprint, no automatic install; paired always 403. |
| se.agentHooks.install | agent_hooks_install_cmd `agent_hooks/commands.rs`; POST /agent-hooks/install (`web/agent_hooks_api.rs`) | W | global | per-call | OPEN | Manage-only CAS install; alias enforces install action. Paired always 403. |
| se.agentHooks.uninstall | agent_hooks_uninstall_cmd `agent_hooks/commands.rs`; POST /agent-hooks/uninstall (`web/agent_hooks_api.rs`) | D | global | per-call | OPEN | Manage-only CAS uninstall; restores byte-exact originals when ownership matches. Paired always 403. |
| se.agentHooks.openFolder | agent_hooks_open_folder_cmd `agent_hooks/commands.rs` | R | global | none | OPEN | Desktop-only fixed custom adapter root; browser facade returns UNSUPPORTED_PLATFORM, no arbitrary path. |
| se.agentHooks.state | agent_hooks_state_cmd `agent_hooks/commands.rs`; GET /agent-hooks/state (`web/agent_hooks_api.rs`) | R | project | none | OPEN-REDACTED | Snapshot/delta cursor, authorization filtered registered projects and tombstones. No native event ring. |
| se.agentHooks.resumePlan | agent_hooks_resume_plan_cmd `agent_hooks/commands.rs`; POST /agent-hooks/resume-plan (`web/agent_hooks_api.rs`) | R | terminal | grant | OPEN | Trusted descriptor, project and owner checks, host snapshot argv; renderer cannot supply recovery argv. |
| se.agentHooks.resumeSpawn | agent_hooks_resume_spawn_cmd `agent_hooks/commands.rs`; POST /agent-hooks/resume-spawn (`web/agent_hooks_api.rs`) | W | terminal | per-call | OPEN | Single-use trusted plan with expected generation; backend capabilities fail closed; no unsafe duplicate spawn. |
| se.agentChat.snapshot | agent_chat_snapshot `agent_chat/commands.rs`; GET /agent-chat/{terminal_id} (`web/agent_chat_api.rs`) | R | terminal | none | OPEN | view scope; host resolves hooked transcript; optional after cursor; redacted, untruncated ACP messages/tools, explicit degraded reason and queue/delivery state; no client file paths |
| se.agentChat.send | agent_chat_send `agent_chat/commands.rs`; POST /agent-chat/{terminal_id}/send (`web/agent_chat_api.rs`) | W | terminal | grant | OPEN | terminal.input scope rechecked before delayed flush; in text; bracketed paste + Enter only at idle/turn_done; busy host queue; delivery only on matching new transcript user echo; timeout unconfirmed, never auto-resend |
| se.agentChat.control | agent_chat_control `agent_chat/commands.rs`; POST /agent-chat/{terminal_id}/control (`web/agent_chat_api.rs`) | W | terminal | grant | OPEN | terminal.input scope; action interrupt writes Esc, stop writes Ctrl-C; never terminates/recreates the PTY |
| se.agentHooks.ingest | POST /agent-hooks/{agent} (`agent_hooks/server.rs`) | W | terminal | none | INTERNAL | native-host-only ingestion for derived se.agentHooks.state; isolated IPv4 loopback listener, per-launch Bearer and PTY identity headers; never mounted on remote HTTP router, no Tauri/plugin twin, no secret-returning API |

---

#### 13. Events (plugin subscriptions)

| event id | source emit (file:line) | payload summary | scope |
|---|---|---|---|
| se.events.agentChatUpdated | `agent-chat-updated` (`agent_chat/mod.rs`, `lib.rs`, `web/router.rs`) | `{terminal_id,cursor}` only; view-authorized clients tail snapshots; never emits prompts/tools/secrets | terminal |
| se.events.agentHookEvent | `agent-hooks-state` (`agent_hooks/mod.rs`, `lib.rs`, `web/router.rs`) | `AgentHookEvent`: sequence, terminal_id, project_id, agent, session_id, kind, at, transcript_path?, tool_name?, ok?; metadata only | project/terminal |
| se.events.acpAgentSpawned | `acp/events.rs:337` | `{sessionId, agentId, runtime, cwd}` | session |
| se.events.acpSessionCreated | `acp/events.rs:339` | `{sessionId, agentId, title?}` | session |
| se.events.acpMessageChunk | `acp/events.rs:341` | streaming content delta | session |
| se.events.acpToolCall | `acp/events.rs:343` | tool-call start (id, title, kind, status) | session |
| se.events.acpToolCallUpdate | `acp/events.rs:345` | status/content patch | session |
| se.events.acpPlanUpdate | `acp/events.rs:347` | plan entries[] | session |
| se.events.acpCommandsUpdate | `acp/events.rs:349` | available slash commands | session |
| se.events.acpModeUpdate | `acp/events.rs:351` | current mode id | session |
| se.events.acpConfigOptionsUpdate | `acp/events.rs:353` | config options list | session |
| se.events.acpPermissionRequest | `acp/events.rs:355` | `{requestId, tool, options}` | session |
| se.events.acpQuestionRequest | `acp/events.rs:362` | `{questionId, prompts, options}` | session |
| se.events.acpPromptComplete | `acp/events.rs:364` | `{stopReason, usage}` | session |
| se.events.acpAgentError | `acp/events.rs:366` | `{code, message}` | session |
| se.events.acpAgentCrashed | `acp/events.rs:371` | `{agentId, exit info}` | profile |
| se.events.acpSessionClosed | `acp/events.rs:374` | `{sessionId}` | session |
| se.events.acpAgentDisconnected | `acp/events.rs:376` | `{agentId}` | profile |
| se.events.acpSessionInfoUpdate | `acp/events.rs:378` | title/metadata changes | session |
| se.events.acpUsageUpdate | `acp/events.rs:380` | token/context usage | session |
| se.events.acpConnectionChanged | `core/acp.rs:160` | Core link state `{connected, …}` | global |
| se.events.acpProjectsChanged | `web/sink.rs:2124` | `{defaultProjectId}` project-mirror change | profile |
| se.events.acpChatHistoryChanged | `web/sink.rs:2145` | history invalidation | project |
| se.events.acpMemoryIndexProgress | `memory_index/commands.rs:52`, emit `:165` | `{vendor, filesSeen, filesTotal}` throttled to 150 ms, always sends first/last | project |
| se.events.acpScheduledTaskDraft | `acp/host_mcp/parent.rs:873`, emit `:881` | `scheduled_task_draft_create` frame | project |
| se.events.conversationHostStatus | `commands.rs:7606` | `()` invalidation | global |
| se.events.conversationAggregate | `commands.rs:7673` (also `7726`, `7793`) | `ConversationAggregateMutationOutcome` data | session |
| se.events.conversationLifecycle | `commands.rs:7809` (also `7835`, `7871`, `7907`, `7948`, `8077`, `8128`) | `ConversationLifecycleOutcome` | session |
| se.events.pluginsChanged | `lib.rs:2558` | plugin snapshot/state event | profile |
| se.events.pluginsConnectionChanged | `lib.rs:2537` (also `2561`) | Core connection state | profile |
| se.events.skillsOperationProgress | `skills/commands.rs:117` | `SkillsOperationStatus{jobId, phase, …}` | project |
| se.events.mcpOauthCompleted | `commands.rs:5456` | `{serverId, success}` | profile |

Note: all `acp:*` events are fan-out through `TauriEventSink` (`web/sink.rs:1803` strips the `acp:` prefix for the WebSocket transport), so one subscription primitive covers desktop and web.

---

#### 14. Counts

| exposure | count |
|---|---|
| OPEN | 143 |
| OPEN-REDACTED | 6 |
| SECRET-EXCLUDED | 1 |
| INTERNAL | 6 |
| **total commands** | **156** |

Classes: R 58, W 89, D 8, SEC 1.

Per-domain command counts: plugin 22, acp/sessions 32, acp_history 9, conversation 17, session_workspace 2, memory 7, skills 12, scheduled 13, ai 5, mcp 22, remote 10, discovery/host 5.

---

#### 15. Not located / unresolved

1. `memory_index_cancel_cmd` (`memory_index/commands.rs:185`) is defined and proxies `memoryCancel`, but is **not** in `invoke_handler` (`lib.rs:4006-4012`) → unreachable. Either register it as `se.memory.cancelBuild` (W, project, none, OPEN) or drop it.
2. `conversation_migration_control` is a sync `pub fn` (`commands.rs:274`, registered `lib.rs:3802`); only the name and location were verified, not its argument struct.
3. `acp_fetch_registry_snapshot` / `acp_install_registry_binary` are in scope by prefix; only their registration lines (`lib.rs:3971-3972`) were read — implementations live in the `acp_registry_snapshot` / `acp_binary_install` modules.
4. `RemoteStatus` was not inspected field by field; the OPEN-REDACTED verdict on `se.remote.rotateCredential` / `se.remote.getStatus` is a requirement to verify, not a proven leak.
5. `plugin_web_read` path confinement to the plugin bundle was not verified.
6. No in-scope command maps to `sshConnection` scope; that scope belongs to part B.

### 清单 B：终端、快捷终端、Git、worktree、搜索、文件、工作区、SSH/SFTP、隧道

### Scope B — Plugin capability catalog (terminal / quick terminal / git / worktree / search / fs / workspace / ssh / sftp / tunnel)

Source of truth: `src-tauri/src/lib.rs:3757-4120` invoke_handler; implementations below. Policy: everything OPEN except credential/secret material.

- **116 commands** in scope → **111 registry ops** after 5 merges.
- Class: R read, W mutates, D destructive/irreversible, SEC secret value.
- Exposure: OPEN, OPEN-REDACTED, SECRET-EXCLUDED, INTERNAL.

#### Terminal (25 commands → 25 rows / 22 ops)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.terminal.spawn | terminal_spawn — commands.rs:442 | W | terminal | none | OPEN | in: `SpawnOptions{shell,cwd,env,conversationId,projectId,cols,rows,program,args,kind}` + `on_data:Channel<Response>`; out: `SpawnedTerminal`+`claim`. Streaming yes. Arbitrary `program`/`args` = direct exec; `kind:"ssh"|"project"` bypasses conversation-scope admission (commands.rs:529). |
| se.terminal.resume | terminal_resume — commands.rs:971 | W | terminal | none | OPEN | in: `TerminalResumeRequest{terminalId,conversationId,lastSeq}` + channel; out: `TerminalResumeGrant{claim,terminal{latestSeq,gap}}` then replay+live bytes. Streaming yes. Rotates the claim generation. |
| se.terminal.list | terminal_list — commands.rs:1076 | R | global | none | OPEN | → `TerminalStatus[]` for **all** live PTYs (id, cwd, project, conversation). Cross-session visibility. |
| se.terminal.attach | terminal_attach — commands.rs:1110 | R | terminal | none | OPEN | in: `(terminalId, claim, lastSeq)` + channel; out: `TerminalAttachResult` + replayed then live raw bytes. Streaming yes. Wrong/missing claim ⇒ generic `UNAUTHORIZED`. |
| se.terminal.screenStatus | terminal_screen_status — commands.rs:1171 | R | terminal | none | OPEN | `(terminalId, claim)` → `ScreenObservationStatus`. Cheap, pollable. |
| se.terminal.inputModes | terminal_input_modes — commands.rs:1192 | R | terminal | none | OPEN | `(terminalId, claim)` → `InputModes` (bracketed paste, mouse, app-cursor). |
| se.terminal.screenRead | terminal_screen_read — commands.rs:1213 | R | terminal | none | OPEN | `(terminalId, claim, maxLines?=80)` → `ScreenReadReply` text. Output is whatever the user's shell printed. |
| se.terminal.watch | terminal_watch — commands.rs:1244 | R | terminal | none | OPEN | `(terminalId, lastSeq)` + channel → replay+live. **No claim check** — trusted-renderer path only (see finding). Plugin variant must re-verify. |
| se.terminal.rotateClaim | terminal_rotate_claim — commands.rs:1441 | W | terminal | per-call | SECRET-EXCLUDED | `(terminalId, claim)` → `{claim}` **new credential value**; old one invalidated. Purpose is credential issuance. |
| se.terminal.revokeClaim | terminal_revoke_claim — commands.rs:1466 | W | terminal | grant | OPEN | `(terminalId, claim)` → `()`. Invalidates a credential but returns nothing; PTY survives. |
| se.terminal.write | terminal_write — commands.rs:1486 | W | terminal | none | OPEN | `(terminalId, data:String)` → `()`. **Raw PTY input** = arbitrary command execution in the owning session; no claim required. |
| se.terminal.resize | terminal_resize — commands.rs:1502 | W | terminal | none | OPEN | `(terminalId, cols, rows, cellWidth?, cellHeight?)` → `()`. Feeds PTY pixel size. |
| se.terminal.setDisplayMode | terminal_set_display_mode — commands.rs:1526 | W | terminal | none | OPEN | `(terminalId, mode:"desktop"|"phone", cols?, rows?)` → `DisplayModeState`. Parks/restores desktop geometry. |
| se.terminal.closeView | terminal_close_view — commands.rs:1563 | W | terminal | none | OPEN | `(terminalId)` → `()`. Drops the output forwarder + pauses cwd/git polling; PTY and claim untouched. |
| se.terminal.terminate | terminal_terminate — commands.rs:1591 | D | terminal | per-call | OPEN | `(terminalId)` → `()`. Kills PTY, removes durable workspace ref, force-kills when already quarantined. |
| se.terminal.terminate | terminal_kill — commands.rs:1791 | D | terminal | per-call | OPEN | **Merged** into `se.terminal.terminate` (deprecated alias, identical body). |
| se.terminal.getCwd | terminal_get_cwd — commands.rs:1801 | R | terminal | none | OPEN | `(terminalId)` → `String?` from the OSC-7 tracker. |
| se.terminal.getGitBranch | terminal_get_git_branch — commands.rs:1811 | R | terminal | none | OPEN | `(terminalId)` → `String?`. |
| se.terminal.getGitStatus | terminal_get_git_status — commands.rs:1821 | R | terminal | none | OPEN | `(terminalId)` → `GitStatus?`. Cheap poll. |
| se.terminal.getExitCode | terminal_get_exit_code — commands.rs:1831 | R | terminal | none | OPEN | `(terminalId)` → `i32?`. |
| se.terminal.setOrphanDetection | terminal_update_orphan_detection — commands.rs:1841 | W | global | grant | OPEN | `{enabled, timeoutMinutes}` → `()`. Host-wide reaping policy for **all** terminals. |
| se.terminal.attach (covered) | terminal_add_renderer_ref — commands.rs:1857 | W | terminal | none | INTERNAL | `{terminalId, rendererId}` → `()`. Renderer bookkeeping; no user-meaningful capability beyond attach. |
| se.terminal.attach (covered) | terminal_remove_renderer_ref — commands.rs:1873 | W | terminal | none | INTERNAL | mirror of the above. |
| se.terminal.setProtected | terminal_set_protected — commands.rs:1894 | W | terminal | none | OPEN | `{terminalId, protected:bool}` → `()`. Exempts a terminal from orphan reaping. |
| se.terminal.attach (covered) | terminal_set_visibility — commands.rs:1914 | W | terminal | none | INTERNAL | `{isVisible}` → `()`. UI polling/kill-deferral hint only. |

#### Quick terminal (8)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.quick.list | quick_terminal_list — quick_terminal_commands.rs:59 | R | profile | none | OPEN | → `QuickTerminalRecord[]` (id, title, target, tabs). |
| se.quick.create | quick_terminal_create — quick_terminal_commands.rs:66 | W | profile | none | OPEN | `{target, title?}` → `QuickTerminalRecord`. Creates the durable record + first shell. |
| se.quick.open | quick_terminal_open — quick_terminal_commands.rs:74 | W | profile | none | OPEN | `{id, cols, rows, terminalId?}` → `QuickTerminalOpened{record, terminalId, claim?, spawned}`. Returns claim when it spawns; reuses a live PTY otherwise. |
| se.quick.rename | quick_terminal_rename — quick_terminal_commands.rs:82 | W | profile | none | OPEN | `{id, title}` → `QuickTerminalRecord`. Title capped 200 chars. |
| se.quick.delete | quick_terminal_delete — quick_terminal_commands.rs:90 | D | profile | per-call | OPEN | `{id}` → `()`. Deletes the record **and** terminates its terminals. |
| se.quick.close | quick_terminal_close — quick_terminal_commands.rs:98 | W | profile | none | OPEN | `{id}` → `QuickTerminalRecord`. Closes without deleting. |
| se.quick.createTerminal | quick_terminal_create_terminal — quick_terminal_commands.rs:106 | W | profile | none | OPEN | `{id, cols, rows}` → `QuickTerminalOpened` (new tab + claim). |
| se.quick.closeTerminal | quick_terminal_close_terminal — quick_terminal_commands.rs:114 | W | profile | none | OPEN | `{id, terminalId}` → `QuickTerminalRecord`. |

#### Git (25)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.git.status | git_get_status — commands.rs:6767 | R | project | none | OPEN | `(cwd)` → `GitStatusDetail[]`. **No path validation**. |
| se.git.diff | git_get_diff — commands.rs:6774 | R | project | none | OPEN | `(cwd, path, staged?=false)` → unified diff string. |
| se.git.stage | git_stage — commands.rs:6785 | W | project | none | OPEN | `(cwd, path)` → `()`. |
| se.git.unstage | git_unstage — commands.rs:6791 | W | project | none | OPEN | `(cwd, path)` → `()`. |
| se.git.stageHunk | git_stage_hunk — commands.rs:6799 | W | project | none | OPEN | `(cwd, path, hunkPatch)` → `()`. Renderer-built unified-diff fragment applied to the index. |
| se.git.unstageHunk | git_unstage_hunk — commands.rs:6806 | W | project | none | OPEN | `(cwd, path, hunkPatch)` → `()`. Reverse-applied. |
| se.git.discard | git_discard — commands.rs:6813 | D | project | per-call | OPEN | `(cwd, path)` → `()`. Deletes untracked files / reverts tracked to HEAD — irreversible. |
| se.git.log | git_get_log — commands.rs:6821 | R | project | none | OPEN | `(cwd, limit?≈200)` → `GitCommit[]`. |
| se.git.commit | git_commit — commands.rs:6828 | W | project | grant | OPEN | `(cwd, summary, description?, amend?=false)` → `()`. Message via temp file; `amend:true` rewrites HEAD. |
| se.git.push | git_push — commands.rs:6848 | W | project | per-call | OPEN | `(cwd)` → `()`. Network write to `origin`; irreversible on the remote. |
| se.git.commitContext | git_get_commit_context — commands.rs:6861 | R | project | none | OPEN | `(cwd)` → `{branch, upstream, ahead, behind, stagedCount, lastCommit}`. |
| se.git.init | git_init — commands.rs:6868 | W | project | grant | OPEN | `(cwd)` → `()`. Creates `.git`. |
| se.git.addIgnoreRules | git_add_ignore_rules — commands.rs:6883 | W | project | none | OPEN | `(cwd, content)` → `()`. Appends to `.gitignore` (idempotent no-op when present). |
| se.git.clone | git_clone — git_clone.rs:374 | W | global | none | OPEN | `(cloneId, url, parentDir, dirName)` + `Channel<GitCloneProgress>` → target path. Streaming yes; long-running; URL scheme allowlist + dir-name sanitizing; stall timeout. |
| se.git.cloneCancel | git_clone_cancel — git_clone.rs:447 | W | global | none | OPEN | `(cloneId)` → `bool` (was running). |
| se.git.branchCheckout | git_checkout_branch — commands.rs:6919 | W | project | grant | OPEN | `(cwd, branch, isRemote?)` → `()`. |
| se.git.branchCreate | git_create_branch — commands.rs:6934 | W | project | none | OPEN | `(cwd, branch, startRef?)` → `()`. Creates **and** checks out. |
| se.git.stashSave | git_stash_save — commands.rs:6955 | W | project | none | OPEN | `(cwd, message?, includeUntracked?)` → `()`. Validates project path. |
| se.git.stashList | git_stash_list — commands.rs:6991 | R | project | none | OPEN | `(cwd)` → `{index, name, message}[]`. |
| se.git.stashApply | git_stash_apply — commands.rs:7033 | W | project | none | OPEN | `(cwd, index)` → `()`. Keeps the stash entry. |
| se.git.stashPop | git_stash_pop — commands.rs:7058 | D | project | per-call | OPEN | `(cwd, index)` → `()`. Applies **and** drops the entry. |
| se.git.stashDrop | git_stash_drop — commands.rs:7083 | D | project | per-call | OPEN | `(cwd, index)` → `()`. Irreversible loss of the stashed work. |
| se.git.branchList | git_branch_list — commands.rs:7108 | R | project | none | OPEN | `(cwd)` → `String[]` (`branch -a --format=%(refname:short)`). |
| se.git.branchCheckout | git_branch_switch — commands.rs:7139 | W | project | grant | OPEN | **Merged** into `se.git.branchCheckout` (same `checkout <name>`; no `isRemote`). |
| se.git.branchCreate | git_branch_create — commands.rs:7163 | W | project | none | OPEN | **Merged** into `se.git.branchCreate` (same `checkout -b`). |

#### Worktree (15)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.worktree.list | worktree_list — commands.rs:1955 | R | project | none | OPEN | `(projectPath)` → `WorktreeInfo[]`; filters bare/detached. |
| se.worktree.create | worktree_create — commands.rs:1976 | W | project | none | OPEN | `(projectPath, name, branch, isNewBranch, startRef?, targetPath?)` → `WorktreeInfo`. Blocking git, path-validated. |
| se.worktree.remove | worktree_remove — commands.rs:2006 | D | project | per-call | OPEN | `(projectPath, worktreePath, force)` → `()`. Deletes the checkout dir + `worktree prune`. |
| se.worktree.branches | worktree_branches — commands.rs:2021 | R | project | none | OPEN | `(projectPath)` → `BranchInfo[]` (name, isRemote, isCurrent, upstream, hasOtherWorktree). |
| se.worktree.checkDirty | worktree_check_dirty — commands.rs:2043 | R | project | none | OPEN | `(worktreePath)` → `DirtyStatus`. |
| se.worktree.removeAllManaged | worktree_remove_all_managed — commands.rs:2054 | D | project | per-call | OPEN | `(projectPath, worktreesJson)` → `RemoveResult[]`. Bulk delete of Se-managed worktrees. |
| se.worktree.parseGitignore | worktree_parse_gitignore — commands.rs:2072 | R | project | none | OPEN | `(projectPath)` → `{dirName, exists}[]`. |
| se.worktree.symlink | worktree_create_symlinks — commands.rs:2094 | W | project | none | OPEN | `(projectPath, worktreePath, symlinkDirsJson)` → `SymlinkResultInfo[]`. Creates symlinks from project root into the worktree. |
| se.worktree.symlink | worktree_ensure_symlinks — commands.rs:2126 | W | project | none | OPEN | **Merged** into `se.worktree.symlink` (`ensure` = create-missing, never remove/overwrite). |
| se.worktree.archive | worktree_archive — commands.rs:2157 | D | project | per-call | OPEN | `(projectPath, worktreePath)` → `()`. Moves the checkout to `<workspace>/archives/`. |
| se.worktree.restore | worktree_restore — commands.rs:2175 | W | project | none | OPEN | `(projectPath, archivePath)` → `()`. Moves it back. |
| se.worktree.mergePreview | worktree_merge_preview — commands.rs:2193 | R | project | none | OPEN | `(worktreePath, targetBranch)` → `MergePreviewInfo{conflicts, changedFiles, detectionMode}`. |
| se.worktree.mergeExecute | worktree_merge_execute — commands.rs:2226 | D | project | per-call | OPEN | `(worktreePath, targetBranch)` → result string. Real merge; conflicts/irreversible index changes. |
| se.worktree.baseBranch | worktree_resolve_base_branch — commands.rs:2241 | R | project | none | OPEN | `(projectPath)` → `BaseBranchInfo` (origin/HEAD → main/master/current, detached flag). |
| se.worktree.copyIncludeFiles | worktree_copy_include_files — commands.rs:2255 | W | project | none | OPEN | `(projectPath, worktreePath)` → `IncludeCopyResult{ran,copied,skipped}`. Copies `.worktree-include` entries; symlink/escape defenses. |

#### Search (6)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.search.rgInfo | search_get_rg_info — commands.rs:3253 | R | global | none | OPEN | `()` → `{sidecarBinaryName, resolvedPath, source, exists}`. Leaks an install path only. |
| se.search.content | search_content — commands.rs:3929 | R | project | none | OPEN | `{scopeRoot, rootPath, query}` → `{results[], truncated, scannedFiles, failedFiles}`. One-shot, capped 100 files × 30 matches. |
| se.search.contentStream | search_content_stream — commands.rs:3266 | R | project | none | OPEN | `{scopeRoot, rootPath, query, searchId}` → `()` + `search-content-batch`/`-done` events. Streaming yes. Query capped; root validated against `scopeRoot`. |
| se.search.cancel | search_content_cancel — commands.rs:3557 | W | project | none | OPEN | `{searchId}` → `()`. Kills the rg child. |
| se.search.fileNames | search_file_names_stream — commands.rs:3575 | R | project | none | OPEN | `{scopeRoot, rootPath, query, searchId, includeIgnored?}` → `()` + `search-file-names-batch`/`-done`. Streaming yes; ignored bucket capped at 20. |
| se.search.cancel | search_file_names_cancel — commands.rs:3905 | W | project | none | OPEN | **Merged** into `se.search.cancel` (same kill-by-searchId registry, different map). |

#### Filesystem / pasteboard / editor / shell (13)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.fs.copyEntries | fs_copy_entries — fs_copy.rs:117 | W | global | none | OPEN | `(sources[], targetDir)` → created paths. Never overwrites (adds " copy", " copy 2"). Arbitrary paths, blocking copy. |
| se.fs.watch | fs_watcher_set_roots — fs_watcher.rs:617 | W | project | none | OPEN | `(roots[])` → `()`; restarts the watcher. Excludes node_modules/target/dist/__pycache__/`.git/{objects,refs,…}`. |
| se.fs.watch (covered) | fs_watcher_subscribe — fs_watcher.rs:608 | W | project | none | INTERNAL | `Channel<Vec<FileChangeEvent>>` install — single process-wide sink (last subscriber wins). |
| se.fs.readAttachmentBytes | read_attachment_bytes — commands.rs:391 | R | global | none | OPEN | `(path)` → raw `ArrayBuffer`. Absolute path, must exist, image extension, ≤10 MB. No project fence. |
| se.fs.readImagePreview | read_image_preview — file_preview.rs; HTTP `GET /api/fs/image` — web/fs_api.rs, web/router.rs | R | global (desktop); project (remote HTTP) | none | OPEN | `{path}` → raw `ArrayBuffer` (Tauri) / image bytes (HTTP), ≤20 MiB with bounded read, canonical regular files only; extension + signature allowlist, accurate raster MIME and HTTP `nosniff`/`no-store`. No HTML/SVG, no system conversion; TIFF/HEIC/HEIF return `UNSUPPORTED_IMAGE_FORMAT` (never text fallback), JFIF uses JPEG and APNG uses PNG MIME/signature validation. Remote HTTP uses provenance + canonical `check_project_boundary`, including symlink escapes; loopback preserves `/fs/read` trusted-local parity. Desktop direct IPC preserves existing arbitrary-file read parity (no project/plugin grant enforcement); **not** a granted-scope confinement guarantee. No events or persistent state; no asset-scope or CSP expansion. Renderer uses bounded MIME-specific `data:` image sources on desktop (existing `img-src` policy) and owned, abort/dispose-revoked `blob:` sources on web. HTTP `/api/fs/info` shares `/fs/read` read-only provenance/boundary policy rather than unconditional loopback-only denial. |
| se.pasteboard.readFilePaths | pasteboard_read_file_paths — file_pasteboard.rs:85 (macOS) / 104 | R | global | none | OPEN | `()` → `{changeCount, paths[]}` from the **general** pasteboard. File URLs only — never clipboard text. macOS-only (empty elsewhere). |
| se.pasteboard.writeFilePaths | pasteboard_write_file_paths — file_pasteboard.rs:79 / 98 | W | global | none | OPEN | `(paths[])` → new changeCount. Overwrites the user's clipboard. macOS-only (errors elsewhere). |
| se.pasteboard.dragFilePaths | drag_pasteboard_file_paths — file_pasteboard.rs:92 / 113 | R | global | none | OPEN | `()` → paths from the **drag** pasteboard. macOS-only. |
| se.shell.detect | detect_shells — lib.rs:487 | R | global | none | OPEN | `()` → `{available: ShellInfo[], default}`. Cached process-wide. |
| se.shell.default | get_default_shell — lib.rs:512 | R | global | none | OPEN | `()` → `ShellInfo`. |
| se.shell.homeDirectory | get_home_directory — lib.rs:517 | R | global | none | OPEN | `()` → `$HOME` / `%USERPROFILE%`. |
| se.editor.listWorkspaces | list_editor_workspaces — commands.rs:5002 | R | global | none | OPEN | `()` → `EditorWorkspaceList` (VS Code / Zed recent workspaces). Discloses local project paths. |
| se.editor.parseWorkspaceFile | parse_code_workspace_file — commands.rs:5010 | R | global | none | OPEN | `{path}` → `EditorWorkspaceList` of folders. Reads any `.code-workspace` path. |

#### SSH / SFTP (19)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.ssh.listProfiles | ssh_list_profiles — commands.rs:4171 | R | sshProfile | none | OPEN-REDACTED | → `SSHProfile[]` from the stored (secret-free) shape: host/user/port/authMethod/privateKeyPath/portForwards + `hasStoredPassword`/`hasStoredPassphrase`. Values never returned. |
| se.ssh.saveProfile | ssh_save_profile — commands.rs:4181 | W | sshProfile | grant | OPEN | `SSHProfile` → `()`. `password`/`passphrase` are **write-only** (host credential backend; separate user-private credentials JSON, not profile JSON or the OS keychain). Recommend a separate SECRET-EXCLUDED `se.ssh.storeCredential`. |
| se.ssh.deleteProfile | ssh_delete_profile — commands.rs:4192 | D | sshProfile | per-call | OPEN | `(profileId)` → `()`. Removes the profile (and its credential-backend entries). |
| se.ssh.importConfig | ssh_import_config — commands.rs:4203 | W | sshProfile | none | OPEN | `()` → imported `SSHProfile[]`. Parses `~/.ssh/config`; identity paths only, no key material. |
| se.ssh.connect | ssh_connect — commands.rs:4214 | W | sshConnection | none | OPEN | `{profileId, password?}` → `SSHConnectionInfo`. **Uses** the host credential backend / accepts an ad-hoc password; returns no secret. |
| se.ssh.disconnect | ssh_disconnect — commands.rs:4259 | W | sshConnection | none | OPEN | `(connectionId)` → `()`. |
| se.ssh.connections | ssh_get_connections — commands.rs:4270 | R | sshConnection | none | OPEN | `()` → `SSHConnectionInfo[]` (id, profile, host, state). |
| se.ssh.portForwardStart | ssh_port_forward_start — commands.rs:4279 | W | sshConnection | grant | OPEN | `{connectionId, id, type:"local"|"remote", localPort, remoteHost, remotePort, label?}` → `ActivePortForward`. Opens a listener. |
| se.ssh.portForwardStop | ssh_port_forward_stop — commands.rs:4324 | W | sshConnection | none | OPEN | `(connectionId, forwardId)` → `()`. |
| se.ssh.createAskpass | ssh_create_askpass — commands.rs:4488 | SEC | global | per-call | SECRET-EXCLUDED | `(password)` → script path. Writes the plaintext password to a 0600 temp `.dat`, plus a cleanup script; 30 s auto-delete. Purpose is secret material. |
| se.sftp.listDir | sftp_list_dir — commands.rs:4339 | R | sshConnection | none | OPEN | `{connectionId, remotePath}` → `SFTPEntry[]` (name, path, type, size, perms, mtime, owner). |
| se.sftp.download | sftp_download — commands.rs:4358 | W | sshConnection | none | OPEN | `{connectionId, remotePath, localPath}` → `()`. Blocking; `ssh-transfer-progress` events. Remote path → **arbitrary local path**. |
| se.sftp.upload | sftp_upload — commands.rs:4394 | W | sshConnection | none | OPEN | `{connectionId, localPath, remotePath}` → `()`. Blocking; progress events. |
| se.sftp.delete | sftp_delete — commands.rs:4430 | D | sshConnection | per-call | OPEN | `{connectionId, remotePath}` → `()`. Recursive-by-type on the remote host. |
| se.sftp.mkdir | sftp_mkdir — commands.rs:4449 | W | sshConnection | none | OPEN | `{connectionId, remotePath}` → `()`. |
| se.sftp.rename | sftp_rename — commands.rs:4468 | W | sshConnection | none | OPEN | `{connectionId, oldPath, newPath}` → `()`. |
| se.sftp.readFile | sftp_read_file — commands.rs:4590 | R | sshConnection | none | OPEN | `{connectionId, remotePath}` → file content as `String`. **No size cap**; binary/UTF-8 lossy. |
| se.sftp.writeFile | sftp_write_file — commands.rs:4609 | W | sshConnection | none | OPEN | `{connectionId, remotePath, content}` → `()`. Overwrites the remote file. |
| se.sftp.createFile | sftp_create_file — commands.rs:4629 | W | sshConnection | none | OPEN | `{connectionId, remotePath}` → `()`. Errors if it exists. |

#### Tunnel / workspace manifest (6)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.tunnel.configGet | tunnel_config_get — remote/tunnel/commands.rs:12 | R | global | none | OPEN-REDACTED | `()` → `TunnelConfigView` (provider, hostname/ports, frp/ssh endpoints, `sshIdentityFile` **path**). `sshPassword`/`sshPrivateKey` never in the view (unit-tested, config.rs:822-833). |
| se.tunnel.configSet | tunnel_config_set — remote/tunnel/commands.rs:22 | W | global | grant | OPEN | `TunnelConfigUpdate` → `TunnelConfigView`. Secret fields are host-credential-backend write-only; the returned view stays redacted. |
| se.tunnel.sshConfigHosts | tunnel_ssh_config_hosts — remote/tunnel/commands.rs:36 | R | global | none | OPEN | `()` → `ParsedSSHProfile[]` from `~/.ssh/config` (host/user/port/identity path). No keys/secrets. |
| se.workspace.manifestGet | workspace_manifest_get — commands.rs:8226 | R | project | none | OPEN | `(projectId)` → `WorkspaceManifest?` (success(None) when absent/degraded). |
| se.workspace.manifestWrite | workspace_manifest_write — commands.rs:8283 | W | project | none | OPEN | `(projectId, basedRevision?, manifest)` → `WriteOutcome{Updated|Conflict}`. Revision-checked; `envVars`/raw claims rejected as VALIDATION_ERROR. |
| se.workspace.manifestDelete | workspace_manifest_delete — commands.rs:8350 | W | project | none | OPEN | `(projectId)` → `()`. Idempotent; never touches PTY/agent state. |

#### EVENTS

| event id | source emit (file:line) | payload summary | rate | scope |
|---|---|---|---|---|
| se.events.terminalOutput | Channel, not app-emit: commands.rs:444 (spawn), 973 (resume), 1114 (attach), 1247 (watch) | raw PTY bytes (`ArrayBuffer`), seq-ordered with ring replay | **very high** (unbounded bytes) — needs explicit subscribe + backpressure/throttle | terminal |
| se.events.terminalExit | terminal_events.rs:195 (`"terminal-exit"`) | `{id, exitCode, signal}` | low (once per PTY) | terminal |
| se.events.terminalCwdChanged | terminal_events.rs:199 | `{terminalId, cwd}` | medium (per prompt/cd) | terminal |
| se.events.terminalGitBranchChanged | terminal_events.rs:206 | `{terminalId, branch}` | low | terminal |
| se.events.terminalGitStatusChanged | terminal_events.rs:213 | `{terminalId, status}` | medium-high (polled per terminal) | terminal |
| se.events.terminalExitCodeChanged | terminal_events.rs:220 | `{terminalId, exitCode}` | per command | terminal |
| se.events.terminalOscTitleChanged | terminal_events.rs:224 | `{terminalId, oscTitle}` | medium | terminal |
| se.events.terminalSpawned | terminal_events.rs:236 | `{terminalId, projectId, conversationId, cwd, cols, rows, shell}` | low | terminal |
| se.events.terminalDisplayModeChanged | terminal_events.rs:253 | `{terminalId, mode, cols, rows}` | low | terminal |
| se.events.searchContentBatch | commands.rs:3412 (`"search-content-batch"`) | `{searchId, results[{filePath, matches[{lineNumber,lineText}]}], truncated}` | high (per 100 ms batch) — throttle by searchId | project |
| se.events.searchContentDone | commands.rs:3273 (also 3294/3320/3349/3367/3541) | `{searchId, truncated, scannedFiles, failedFiles, code?, error?}` | low (terminal event) | project |
| se.events.searchFileNamesBatch | commands.rs:3802 (also 3829/3840) | `{searchId, files[{path, ignored}], truncated?}` | high (batches of 25) | project |
| se.events.searchFileNamesDone | commands.rs:3584 (also 3603/3645/3673/3700/3890) | `{searchId, truncated, totalFiles, code?, error?}` | low | project |
| se.events.fsChange | fs_watcher.rs:608-613 (`Channel<Vec<FileChangeEvent>>`), payload `FileChangeEvent` at fs_watcher.rs:96 | `[{kind:"add"|"change"|"unlink", path}]`, coalesced | bursty/high on builds — coalescer + root exclusion already; still needs per-subscriber throttle | project |
| se.events.sshConnectionStatusChanged | ssh/connection.rs:819 (`"ssh-connection-status-changed"`) | `SSHConnectionInfo` (id, host, state) | low | sshConnection |
| se.events.sshPortForwardStatusChanged | ssh/port_forward.rs:406 (also 242) | `(connectionId, ActivePortForward)` | low | sshConnection |
| se.events.sshTransferProgress | ssh/sftp.rs:175/191 (download), 258/274 (upload) (`"ssh-transfer-progress"`) | `{connectionId, remotePath, localPath, bytesTransferred, totalBytes, direction, status, error?}` | **high** (per chunk) — throttle to N/s | sshConnection |
| se.events.gitCloneProgress | git_clone.rs:368 (`on_progress.send`, `GitCloneProgress` @ git_clone.rs:91) | progress line/percent per clone | medium (per progress line) | project |
| se.events.quickTerminalsChanged | quick_terminal_migration.rs:196, const @ :184 (`"quick-terminals-changed"`) | `()` | low (migration only today) | profile |
| se.events.terminalCoreRestarted | lib.rs:2828, const @ lib.rs:2482 (`"terminal:core_restarted"`) | live terminal ids that survived the core restart | low | global |

#### Counts per exposure (116 commands)

| exposure | count |
|---|---|
| OPEN | 108 |
| OPEN-REDACTED | 2 |
| INTERNAL | 4 |
| SECRET-EXCLUDED | 2 |
| **total** | **116** |

Class totals: R 38 · W 68 · D 8 · SEC 2. Confirm: per-call 16 · grant 9 · none 91.

#### Could not locate / negative results

- **No backend event** for tunnel start/stop progress, worktree create/archive/merge progress, or workspace-manifest mutation. Only `git_clone` has a progress channel.
- **No cancellation command** for `sftp_upload`/`sftp_download` (only `git_clone_cancel` and the two search cancels exist).
- **No app-level `git-status-changed`** independent of terminals: git status changes reach the renderer only via `terminal-git-status-changed` (polled) or an explicit `git_get_status` call.
- **No `fs_*` read/write/list/delete Tauri commands** in scope besides `fs_copy_entries`; the broader `src/web/fs_api.rs` surface is an HTTP router, not `generate_handler!` entries.
- Grepped `src-tauri/**/*.rs` for `\.emit\(\s*"` and for `"<domain>[:_-]` event names — 219 raw matches, of which only the ones listed above are in scope; the rest are brand-migration, plugin, and conversation events (Scope A).

### 清单 C：浏览器、数据迁移、安全存储、日志、系统，以及前端独有能力

### Scope C — Complete Plugin Capability Catalog (Se Manager / termul)

**Repo:** `/Users/qs/project/me/termul` · **Read-only planning; no files changed.**

#### 0. Method and correction to the dispatch

The dispatch points at `/Users/qs/project/me/cea-jcode/.qin-code/scratch/tauri-commands.txt`. **That file does not exist** (searched `**/tauri-commands*` across `/Users/qs/project/me`, and listed both `.qin-code/scratch` trees). The authoritative command registry is the `generate_handler!` list in `src-tauri/src/lib.rs:3759-4115`. I derived scope C by subtracting the A/B prefixes from that list and then verified each scope-C name is present at a concrete handler line.

```mermaid
flowchart LR
  R[Renderer stores] -->|persistenceApi| S[(settings/app, themes/custom, projects, ui keys)]
  R -->|invoke| T[Tauri commands]
  T --> M[Rust managers]
  R -->|renderer_provider_publish (proposed)| B[Bridge]
  B --> P[plugin-core event hub]
  T -->|plugin_publish_host_context| P
```

#### 1. Part 1 — Tauri commands not covered by A/B

Legend — class: **R** read, **W** write, **D** destructive, **SEC** secret-bearing. Exposure: OPEN / OPEN-REDACTED / SECRET-EXCLUDED / INTERNAL.

##### 1.1 Browser tabs (`commands.rs`, registered `lib.rs:3832-3852`)

| op id | source (file:line) | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| browser_tab.create | src-tauri/src/commands.rs:2374 | W | browser tab (own webview) | none | OPEN | rust-main | Creates a child webview at an arbitrary URL; arbitrary-URL navigation is the main abuse surface. |
| browser_tab.navigate | src-tauri/src/commands.rs:2388 | W | one tab | none | OPEN | rust-main | URL is unvalidated; keep host allowlist at provider. |
| browser_tab.resize | src-tauri/src/commands.rs:2401 | W | one tab | none | OPEN | rust-main | Geometry only. |
| browser_tab.show | src-tauri/src/commands.rs:2414 | W | one tab | none | OPEN | rust-main | |
| browser_tab.hide | src-tauri/src/commands.rs:2426 | W | one tab | none | OPEN | rust-main | |
| browser_tab.destroy | src-tauri/src/commands.rs:2438 | D | one tab | none | OPEN | rust-main | Only the plugin's own tab id should be acceptable. |
| browser_tab.go_back | src-tauri/src/commands.rs:2450 | W | one tab | none | OPEN | rust-main | |
| browser_tab.go_forward | src-tauri/src/commands.rs:2462 | W | one tab | none | OPEN | rust-main | |
| browser_tab.reload | src-tauri/src/commands.rs:2474 | W | one tab | none | OPEN | rust-main | |
| browser_tab.open_devtools | src-tauri/src/commands.rs:2496 (debug) / 2508 (release stub) | W | one tab | none | OPEN | rust-main | Release build returns BROWSER_TAB_OPEN_DEVTOOLS_DISABLED; harmless but pointless to expose. |
| browser_tab.inject_annotation | src-tauri/src/commands.rs:2520 | W | one tab | none | OPEN | rust-main | Injects a script into the page; script text is fixed host-side. |
| browser_tab.remove_annotation_overlay | src-tauri/src/commands.rs:2533 | W | one tab | none | OPEN | rust-main | |
| browser_tab.inject_annotation_markers | src-tauri/src/commands.rs:2548 | W | one tab | none | OPEN | rust-main | Accepts arbitrary annotations JSON injected into the page. |
| browser_tab.update_annotation_marker_selection | src-tauri/src/commands.rs:2569 | W | one tab | none | OPEN | rust-main | |
| browser_tab.report_url | src-tauri/src/commands.rs:2585 | W | one tab | none | INTERNAL | rust-main | Injected-JS plumbing; re-emits `browser-tab-navigated`. Covered by browser_tab.navigate. |
| browser_tab.report_loaded | src-tauri/src/commands.rs:2606 | W | one tab | none | INTERNAL | rust-main | Plumbing; emits `browser-tab-loaded`. Covered by browser_tab.reload/navigate. |
| browser_tab.report_region_captured | src-tauri/src/commands.rs:2627 | W | one tab | none | INTERNAL | rust-main | Plumbing; emits `browser-tab-region-captured`. |
| browser_tab.report_title | src-tauri/src/commands.rs:2666 | W | one tab | none | INTERNAL | rust-main | Plumbing; emits `browser-tab-title-changed`. |
| browser_tab.report_element_captured | src-tauri/src/commands.rs:2686 | W | one tab | none | INTERNAL | rust-main | Plumbing; emits `browser-tab-element-captured` with page text. Covered by the annotation UX. |
| browser_tab.report_annotation_marker_clicked | src-tauri/src/commands.rs:2745 | W | one tab | none | INTERNAL | rust-main | Plumbing; emits `browser-tab-annotation-marker-clicked`. |

##### 1.2 Data migration (`commands.rs`, registered `lib.rs:3902-3907`)

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| data_migration.get_version | src-tauri/src/commands.rs:4064 | R | app DB | none | OPEN | rust-main | Schema version string. |
| data_migration.get_schema_info | src-tauri/src/commands.rs:4072 | R | app DB | none | OPEN | rust-main | Current + target. |
| data_migration.get_history | src-tauri/src/commands.rs:4080 | R | app DB | none | OPEN | rust-main | Applied migration records. |
| data_migration.get_registered | src-tauri/src/commands.rs:4088 | R | app DB | none | OPEN | rust-main | Static registry. |
| data_migration.run_migrations | src-tauri/src/commands.rs:4096 | W/D | app DB (global) | per-call | OPEN | rust-main | Global, not plugin-scoped; a plugin could force a schema bump mid-session. |
| data_migration.rollback | src-tauri/src/commands.rs:4104 | D | app DB (global) | per-call | OPEN | rust-main | Rollback to arbitrary version — highest-risk item in this group. |

##### 1.3 Secrets (`secure_storage.rs`, registered `lib.rs:3935-3937`)

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| secure_storage.set | src-tauri/src/secure_storage.rs:174 | W/SEC | credential backend, arbitrary key | n/a (excluded) | SECRET-EXCLUDED | rust-main | Writes a raw secret value. No plugin path. |
| secure_storage.get | src-tauri/src/secure_storage.rs:182 | R/SEC | credential backend, arbitrary key | n/a | SECRET-EXCLUDED | rust-main | Returns the raw value; keyring_get (85) falls back to the legacy brand service and copies forward. |
| secure_storage.delete | src-tauri/src/secure_storage.rs:194 | D/SEC | credential backend, arbitrary key | n/a | SECRET-EXCLUDED | rust-main | Deletes a credential; do not expose even as "delete my key". |

**What it stores (checked):** the module's own doc comment states the legacy fallback exists so users do not lose "stored project environment variables" (`secure_storage.rs:75-84`). Renderer access is the thin wrapper `src/renderer/lib/tauri-secure-storage-api.ts:37/66/89`. SSH passwords/keys are separate (`src-tauri/src/ssh/credential_store.rs`). Policy consequence: env-var injection into a pty must stay entirely host-side; a plugin may reference a variable name but never read it.

##### 1.4 Brand / storage handoff

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| webview_storage_handoff.capture | src-tauri/src/webview_storage_handoff.rs:380 | W/SEC | app-data handoff file | n/a | SECRET-EXCLUDED | rust-main | Takes the renderer's entire localStorage map; the filter is a key-prefix filter, not a secret filter. |
| webview_storage_handoff.pending | src-tauri/src/webview_storage_handoff.rs:398 | R/SEC | app-data handoff file | n/a | SECRET-EXCLUDED | rust-main | Returns those entries verbatim. |
| webview_storage_handoff.mark_consumed | src-tauri/src/webview_storage_handoff.rs:410 | W | handoff file | none | INTERNAL | rust-main | Bookkeeping only; covered by handoff.pending. |
| brand_migration.detect_legacy_brand_data | src-tauri/src/migration_detect.rs:612 | R | legacy roots on disk | none | OPEN | rust-main | Path/metadata scan only; returns signal kinds. |
| brand_migration.run | run_brand_migration `src-tauri/src/migration_run.rs:731` | W/D | app-data tree | per-call | OPEN | rust-main | Bulk copy; per-root status, journal-mediated, legacy roots kept (FORBID-05). |
| brand_migration.last_run | src-tauri/src/migration_run.rs:766 | R | journal file | none | OPEN | rust-main | Never errors; unreadable journal = None. |

##### 1.5 Core runtime

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| core.runtime_status | src-tauri/src/lib.rs:1873 | R | all cores + terminal/agent inventory | none | OPEN | rust-main | Returns pids, start times, build ids, active terminal list and agent ids — topology disclosure, not a secret. |
| core.component_identities | src-tauri/src/lib.rs:1730 | R | cores | none | OPEN | rust-main | Probes each endpoint's hello ack. |
| core.restart | restart_core `src-tauri/src/lib.rs:1926` | W/D | one core role | per-call | OPEN | rust-main | Terminates every resource owned by that core (terminals, agents). |
| core.component_identities | component_runtime_identities `src-tauri/src/lib.rs:1730` | R | all cores | none | OPEN | rust-main | 由根节点补录（2026-11-04 覆盖检查发现漏项）。返回 GUI、acp/terminal/plugin core 的 build id 和运行身份，不含密钥。 |
| updater.fetchChannelManifest | updater_fetch_channel_manifest `src-tauri/src/updater_api.rs:128` | R | global | none | OPEN | rust-main | 由根节点补录。in `channel`（stable/insider/nightly）；从服务端拉取更新清单 JSON，需要联网。 |

##### 1.6 Menus, keybindings, i18n, quit behaviour

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| app_menu.suspend | suspend_app_menu `src-tauri/src/lib.rs:537` | W | native menu (global) | none | OPEN | rust-main | Removes the app menu; a plugin could hide it. |
| app_menu.restore | restore_app_menu `src-tauri/src/lib.rs:557` | W | native menu (global) | none | OPEN | rust-main | Rebuilds it. |
| ui.set_native_language | set_native_ui_language `src-tauri/src/lib.rs:576` | W | global UI chrome + tray | none | OPEN | rust-main | Serialized against concurrent calls (lib.rs:226). |
| keybindings.load_schemes | keybinding_schemes_load `src-tauri/src/keybinding_schemes.rs:92` | R/W | scheme dir | none | OPEN | rust-main | `ensure_dir` makes the directory; otherwise a disk read. |
| quit.set_end_core_sessions | set_quit_ends_core_sessions `src-tauri/src/lib.rs:1624` | W | global preference | none | OPEN | rust-main | Stores an atomic; only meaningful on the next quit. |

##### 1.7 Logs and diagnostics

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| logs.set_retention_days | set_log_retention_days `src-tauri/src/lib.rs:660` | W/D | log dir | per-call | OPEN | rust-main | Deletes archives older than `days` (logging.rs:154). |
| logs.reveal_dir | reveal_log_dir_command `src-tauri/src/lib.rs:678` | W | OS file manager | none | OPEN | rust-main | Opens Finder/Explorer at the log dir. |
| logs.export_file | export_log_file_command `src-tauri/src/lib.rs:683` | R | log file → user path | none | OPEN-REDACTED | rust-main | Save dialog; raw log content. |
| logs.copy_contents | copy_log_contents_command `src-tauri/src/lib.rs:688` | R | log file → clipboard | none | OPEN-REDACTED | rust-main | Raw content to the system clipboard. |
| logs.export_default | export_log_to_default_command `src-tauri/src/lib.rs:693` | R | log file → default dir | none | OPEN-REDACTED | rust-main | Raw content to a fixed path. |
| logs.frontend_error | log_frontend_error `src-tauri/src/commands.rs:7221` | W | log sink | none | OPEN | rust-main | Renderer-supplied message; a plugin can write arbitrary text into the log. |

##### 1.8 macOS privacy

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| macos.permissions_report | macos_permissions_report_command `src-tauri/src/lib.rs:703` | R | TCC state | none | OPEN | rust-main | May trigger OS prompts for the ids passed in `active`; spawns `codesign`. |
| macos.open_privacy_pane | macos_open_privacy_pane_command `src-tauri/src/lib.rs:723` | W | System Settings | none | OPEN | rust-main | Takes a probe id, not a URL — the URL map lives in `macos_permissions`. |

##### 1.9 Shell / environment

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| shell.detect | src-tauri/src/lib.rs:487 | R | host env | none | OPEN | rust-main | Enumerates installed shells. |
| shell.get_default | src-tauri/src/lib.rs:512 | R | host env | none | OPEN | rust-main | |
| env.home_dir | src-tauri/src/lib.rs:517 | R | host env | none | OPEN | rust-main | Discloses the user's home path; low risk, prefer folding into shell.get_default. |

##### 1.10 Residual (outside A/B prefixes, flagged)

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| ~~agent_registry.fetch~~ | src-tauri/src/lib.rs:3830 | — | — | — | (重复) | — | 与清单 A 的 `se.agents.fetchRegistry` 是同一个命令，以 A 为准，本行不计数。 |

#### 2. Part 2 — Renderer-owned capabilities (no Rust command)

All of these persist through `persistenceApi` (`src/renderer/lib/persistence-api.ts:15`), which is the Tauri plugin-store file on desktop and the server JSON store on web. Provider = **renderer** (the GUI provider). The proposed route is a single state service with a per-document allowlist: `state.get(doc)`, `state.patch(doc, ops)`, `state.subscribe(doc)` — never raw store access.

##### 2.1 Projects (`src/renderer/stores/project-store.ts`)

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| projects.create | project-store.ts:197 (`addProject`, sig :92) | W | project list | none | OPEN | renderer | `addProject(name, color, path)` → Project (:205). |
| projects.update | project-store.ts:223 | W | one project | none | OPEN | renderer | Rename, color, default shell, project settings all land here (`Partial<Project>`). |
| projects.delete | project-store.ts:229 | D | one project | per-call | OPEN | renderer | Removes from the list; does not touch disk. |
| projects.archive | project-store.ts:257 | W | one project | none | OPEN | renderer | Hides from the active list. |
| projects.restore | project-store.ts:276 | W | one project | none | OPEN | renderer | Inverse of archive. |
| projects.select | project-store.ts:159 | W | active selection | none | OPEN | renderer | Changes what the whole UI shows; scope to the plugin's own action. |
| projects.move_to_group | project-store.ts:409 | W | project + group | none | OPEN | renderer | Optional target index. |
| projects.reorder | project-store.ts:295 | W | project list | none | OPEN | renderer | Full id array rewrite. |
| projects.groups.add | project-store.ts:353 | W | groups | none | OPEN | renderer | Returns the new group id. |
| projects.groups.rename | project-store.ts:397 | W | one group | none | OPEN | renderer | |
| projects.groups.remove | project-store.ts:367 | D | one group | per-call | OPEN | renderer | `deleteProjects` flag decides whether members die too. |
| projects.groups.reorder | project-store.ts:458 / 473 | W | groups | none | OPEN | renderer | Group order and per-group project order. |

##### 2.2 App settings, themes, shortcuts

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| mcp.gatewayModes | stores/mcp-gateway-store.ts; shared/types/mcp-gateway.types.ts | R/W | `mcp/gateway-modes` preference doc | grant | OPEN | renderer | Allowlist `{internal,aggregator}` values grouped/entry/direct. Legacy `mcp/gateway-mode` migrates only to internal; aggregator defaults grouped. Never exposes connection/token/runtime state or changes authorization. |
| settings.get | app-settings-store.ts:14 (+ `settings` :6) | R | settings/app doc | none | OPEN | renderer | Single settings blob. |
| settings.set | app-settings-store.ts:25 (`updateSettings`) / :20 (`updateSetting`) | W | settings/app doc | none | OPEN | renderer | Field-whitelisted patch. |
| settings.reset | app-settings-store.ts:30 | W | settings/app doc | per-call | OPEN | renderer | Whole-blob revert to defaults. |
| theme.set | theme-picker-store.ts:15-58 (`applyColorTheme` :47, :53) + hooks/use-color-theme.ts:19-44 | W | theme selection | none | OPEN | renderer | Resolves light/dark effective id. |
| theme.list | theme-picker-store.ts:4-14 (state) | R | theme catalog | none | OPEN | renderer | Built-ins plus custom list. |
| theme.custom.write | lib/persistence-api.ts:26 (`themes/custom`) | W | themes/custom doc | per-call | OPEN | renderer | Import/delete a user theme rewrites the list; validate the token schema before writing. |
| shortcuts.set | keyboard-shortcuts-store.ts:45 (`updateShortcut`) | W | shortcuts doc | none | OPEN | renderer | Single binding override. |
| shortcuts.reset | keyboard-shortcuts-store.ts:46 / :47 | W | shortcuts doc | none | OPEN | renderer | One or all. |
| shortcuts.scheme.apply | keyboard-shortcuts-store.ts:51 (`applyScheme`) | W | shortcuts doc | none | OPEN | renderer | Switches the whole scheme. |
| shortcuts.plugin.register | keyboard-shortcuts-store.ts:33 / :38 | W | plugin shortcut table | none | OPEN | renderer | Plugin-declared bindings with generation fencing; plugin-facing already. |

##### 2.3 Layout, navigation, chrome

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| ui.rail.get/set | navigation-store.ts:19 (key `settings/activity-rail`), :58 hydrate | R/W | rail layout doc | none | OPEN | renderer | |
| ui.rail.move | navigation-store.ts:66 / :72 | W | rail layout | none | OPEN | renderer | Move by delta or absolute index. |
| ui.rail.pin | navigation-store.ts:78 | W | rail layout | none | OPEN | renderer | |
| ui.rail.dividers | navigation-store.ts:84 / :88 / :92 | W | rail layout | none | OPEN | renderer | Insert/remove/toggle. |
| ui.sidebar.visibility | sidebar-store.ts:12 / :16 (state :4) | R/W | session UI | none | OPEN | renderer | |
| ui.context_bar.set | context-bar-settings-store.ts:17 (`toggleElement`) / :22 | W | context bar settings | none | OPEN | renderer | Element toggles (git branch, exit code, cwd…). |
| workspace.tab.close | workspace-store.ts:608 (state :146-220) | W | one pane tab | none | OPEN | renderer | Returns the closed tab for undo. |
| workspace.layout | workspace-store.ts:146-220 | R/W | pane tree | none | OPEN | renderer | Split sizes/panes; read via state snapshot. |

##### 2.4 Editor, terminal, explorer

| op id | source | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| editor.open_file | editor-store.ts:118 (sig :97, action :488) | R/W | one file in editor | none | OPEN | renderer | Reads via fs_* (A/B) and opens a tab; path checks belong to fs.read. |
| terminal.open/select | terminal-store.ts:169 (`addTerminal`) / :166 (`selectTerminal`) | W | terminal list | none | OPEN | renderer | Spawn itself is `terminal_spawn` (A/B). |
| terminal.close/reopen | terminal-store.ts:191 / :192 / :193 | W | one terminal view | none | OPEN | renderer | View lifecycle, not process kill. |
| terminal.quick.adopt | terminal-store.ts:183 (`adoptQuickTerminal`) / :190 (`forgetQuickTerminal`) | W | quick terminal UI | none | OPEN | renderer | UI-side identity for a quick terminal. |
| explorer.roots | file-explorer-store.ts:109 (state), :72-77 Root | R | explorer state | none | OPEN | renderer | Per-project roots. |
| explorer.expand/select | file-explorer-store.ts:118 / :126-127 | W | explorer state | none | OPEN | renderer | |
| explorer.clipboard | file-explorer-store.ts:128 (+ `isRejectedMove` :35) | W | explorer clipboard | none | OPEN | renderer | Copy/cut intent only; the actual fs op is fs_copy (A/B). |
| explorer.search | file-explorer-store.ts:134-146 | R/W | explorer search | none | OPEN | renderer | Search results are UI state. |

#### 3. Part 3 — Cross-cutting

##### 3.1 Plugin-facing event model for renderer-originated events

Today the renderer can only *listen* (`lib/host-events.ts:16-20`, `lib/browser-api.ts:138-168`, `tauri-terminal-api.ts:142-257`, `lib/tauri-keyboard-api.ts:51-76`). Nothing pushes renderer state changes to plugin-core; the only host→plugin channel is the host-context pair `plugin_publish_host_context` / `plugin_heartbeat_host_context` (`src-tauri/src/lib.rs:3796-3797`).

Proposed contract (renderer provider → bridge → plugin-core hub):

| topic | emitted on | payload | source |
|---|---|---|---|
| `project.changed` | add/update/delete/archive/group/reorder | `{revision, changedId, kind}` | project-store.ts:197-473 |
| `project.active.changed` | select | `{revision, projectId}` | project-store.ts:159 |
| `settings.changed` | any settings write | `{revision, keys[]}` (values never in the event) | app-settings-store.ts:18-30 |
| `theme.changed` | applyColorTheme / custom write | `{revision, themeId, mode}` | theme-picker-store.ts:47, persistence-api.ts:26 |
| `shortcuts.changed` | binding/scheme write | `{revision, schemeId}` | keyboard-shortcuts-store.ts:44-51 |
| `layout.changed` | rail/sidebar/context-bar/tabs | `{revision, area}` | navigation-store.ts:58-92, sidebar-store.ts:12, context-bar-settings-store.ts:17, workspace-store.ts:608 |

Rules:
1. Events carry **revision + identifiers**, never document bodies — the consumer must call `state.get(doc)` under its grant.
2. Subscribe = one `state.subscribe(doc)` with a snapshot-on-subscribe reply (`{revision, snapshot}`), so a missed event is recoverable.
3. Revision is monotonic per document and comes from the renderer store, so coalescing is safe.
4. Events are delivered through plugin-core (same envelope as host-context), so a plugin never opens a Tauri `listen` directly.
5. Secret-bearing documents (`secure_storage`, `webview_storage_handoff`) emit no events at all.

##### 3.2 Commands present in src-tauri but not in invoke_handler

Verified structurally: registration is exclusively the `generate_handler!` list at `lib.rs:3759-4115`. Spot-check of every scope-C item found it registered. Named non-commands that look like commands but are not:
- `restart_core_on_request` (`lib.rs:2758`) — internal helper.
- `get_default_shell_info` (`lib.rs:729`) — internal helper.
- `logging::set_log_retention_days` (`logging.rs:154`) — library function; the command wrapper is `lib.rs:660`.
- `web/router.rs`, `web/*_api.rs` — axum/web-server handlers, not Tauri commands.
- `se_mcp_main.rs`, `server_main.rs`, `terminal_core_main.rs`, `mcp_fixture_bin.rs` — separate binaries.
- `migration_detect`/`migration_run` — only the three exported commands are registered (`lib.rs:4109-4111`).

No orphan scope-C command was found. An exhaustive `#[tauri::command]` ↔ handler diff was **not** run (see open issues).

#### 4. Counts

| exposure | count |
|---|---|
| OPEN | 70 |
| OPEN-REDACTED | 3 |
| SECRET-EXCLUDED | 5 |
| INTERNAL | 7 |
| **total** | **85** |

| provider | count |
|---|---|
| rust-main | 56 |
| renderer | 29 |
| mcp-core | 0 |
| plugin-core | 0 |
| **total** | **85** |

Per-group: browser 20 (14 OPEN, 6 INTERNAL) · data_migration 6 · secure_storage 3 (all excluded) · handoff/brand 6 (2 excluded, 1 internal, 3 open) · core 3 · menus/lang/keybinding 5 · logs 6 · macos 2 · shell 3 · residual 1 · renderer 29.

#### 5. Not found / not verified

- `tauri-commands.txt` — does not exist in either tree.
- Notifications/toasts store — no store under `src/renderer/stores` matched; surface not catalogued.
- Command palette pinned/recent — files exist (`pinned-commands-store.ts`, `recent-commands-store.ts`) but no line anchors were read.
- Per-project default shell field — `ProjectSettings` page exists; the exact field/write path was not verified (`projects.update` is the presumed write path).
- `src/renderer/lib/themes` directory listing — behavior inferred from `apply-color-theme.test.ts:76`; file inventory not enumerated.
- Exhaustive unregistered-command diff — not run.

### 宿主 MCP 能力同步（2026-11-07）

本增量补充清单 A 的 MCP 部分；既有 A/B/C 清单、事件、来源和历史 explorer 统计仍保留。前轮合并统计新增 **3 个 Tauri 命令：OPEN +2、OPEN-REDACTED +1**；本轮纯管理预览另增 **1 个 OPEN 命令**，该补充累计 4 个（OPEN +3、OPEN-REDACTED +1）；HTTP twin、私有 bridge、provider 子工具和复用的 renderer 状态操作不重复计数。这里的 OPEN/confirm 是**插件能力目录契约**，不是声称当前 MCP 或 Tauri 原始入口已经接入插件级确认、审计、按插件身份隔离的运行时。

#### 新增公开操作（清单 A / MCP）

| op id | source cmd (file:line) | class | scope | confirm | exposure | provider | notes |
|---|---|---|---|---|---|---|---|
| se.mcp.hostAccessGet | mcp_host_access_get `src-tauri/src/commands.rs:5652`；注册 `src-tauri/src/lib.rs:4059` | R | global（宿主授权管理元数据） | none | OPEN-REDACTED | rust-main | `()` → `McpHostAccessView{policy,profiles,projects,hostMode}`；非长任务。profiles 为脱敏候选（profileId/name/host/port/username/authMethod/tags/fingerprint），projects 为 id/name；不返回密码、passphrase、私钥、bearer 或 bridge descriptor/token。policy 的项目范围、remoteRoots/localRoots 等仍属敏感 metadata，不得当作公开无敏感信息；插件投影须按调用方 scope 过滤并最小化路径/主机/用户信息，不直接转发本机管理员完整视图。  v2 policy 包含全局 `ssh.commandDeny` 与 `ssh.profiles[profileId].commandDeny` 数组；规则 `{id,kind:exact或contains或regex,pattern}`，规则文本亦须按调用方权限最小化投影。 |
| se.mcp.hostAccessPut | mcp_host_access_put `src-tauri/src/commands.rs:5662`；注册 `src-tauri/src/lib.rs:4060` | W | global（所有本宿主 MCP 客户端共享的授权策略） | per-call | OPEN | rust-main | `{expectedRevision,policy}` → 已保存 `HostAccessPolicy`；非长任务。修改 enabled/workspace/projectIds/includePaths、SSH profile/operations/roots/posixJobs 与 profile fingerprints，属于**高权限委托/授予权限**，不是普通 grant 写偏好；每次必须本机确认，不能让调用者自授权。revision CAS 冲突返回 `REVISION_CONFLICT`，先重读而非盲目覆盖；授权回显不含凭据值。  v2 同时保存全局/profile `commandDeny`；非法规则整份拒绝、fail closed；普通 UI 授权编辑必须保留两层规则。全局与 profile deny 取并集，默认全 13 operations 仍不能绕过。 |
| se.mcp.hostAccessPreview | mcp_host_access_preview(request) `src-tauri/src/commands.rs:5679`；注册 `src-tauri/src/lib.rs:4066` | R | global（请求中的 sshProfile） | none | OPEN | rust-main | `{profileId,operation:execute或job_start,command,policy?}` → `{outcome:matched或not_matched,matches:[{id,source:global或profile}],policyRevision,draft,blockedBy?}`；纯管理预览，不把输入命令当执行。不读取 credentials、不联网、不占用/排队 worker、不创建 jobs、不持久化；草稿不保存、不授权；非法规则 IPC 错误；无原始 command/pattern 回显；未命中不等于可执行。Web 仅 LocalOperator，PublicTunnel 403。 |
| se.mcp.hostCall | mcp_host_call `src-tauri/src/commands.rs:5675`；注册 `src-tauri/src/lib.rs:4061` | W | global → project / sshProfile / job / local+remote path（operation-specific） | per-call | OPEN | rust-main | `{provider:"workspace"或"ssh",tool,arguments}` → 对应操作结果。仅复用宿主明确授权的 workspace/SSH，宿主读取并使用凭据，不返回凭据；通用入口同时包含执行、文件写入/传输和取消，保守逐次确认，不能仅按 provider 或只读工具 annotation 放行。`job_start` 返回 jobId，`job_status/job_list` 查询；execute/test/传输可能耗时，不能宣称全部调用立即完成。禁止从 arguments 接收 `callerId/grant/confirmed` 充当权限。 |

前轮 Tauri 三命令均已注册；本轮追加 preview 后共四命令，不是未接线计划。`commands.rs` 调用同一 `HostCapabilityService`；权限、scope 与凭据使用边界由 `src-tauri/src/mcp_core/{host_service,host_policy,host_capabilities,host_adapters,workspace,host_tools}.rs` 实施。目录中的 per-call 是插件适配层要求，尚不能用原始 IPC 成功作为“插件确认已生效”的证据。

#### HTTP twin：已存在的 shared-live / standalone 路由

已读取 `src-tauri/src/web/mcp_host_api.rs`，并核对 `src-tauri/src/web/router.rs:79-93`；因此以下记录为**已接线 HTTP twin**，不标成 future/unimplemented。它们不另计 Tauri 命令或独立 renderer 能力。

| HTTP surface | source | covering op | exposure / confirm | actual gate and result |
|---|---|---|---|---|
| GET /mcp/host/access | `src-tauri/src/web/mcp_host_api.rs:59` | se.mcp.hostAccessGet | OPEN-REDACTED / none | 要求 `RemotePrincipal` 且 `IngressProvenance::LocalOperator`；返回 `IpcBody<McpHostAccessView>`，无凭据。 |
| PUT /mcp/host/access | `src-tauri/src/web/mcp_host_api.rs:76` | se.mcp.hostAccessPut | OPEN / per-call | 同一 provenance 门禁；body `{expectedRevision,policy}`，返回策略或带错误码的 IpcBody（包括 `REVISION_CONFLICT`），不假定冲突一定是 HTTP 409。 |
| POST /mcp/host/access/preview | `src-tauri/src/web/mcp_host_api.rs:120`；`src-tauri/src/web/router.rs:83,95` | se.mcp.hostAccessPreview | OPEN / none | 两种 provenance 路径均注册，handler 仍 `require_local`；body 为 request，结果为预览或 IPC 错误。无 credentials/network/worker/jobs/persist，不回显原始 command/pattern，PublicTunnel 403。 |
| POST /mcp/host/call | `src-tauri/src/web/mcp_host_api.rs:96` | se.mcp.hostCall | OPEN / per-call | 同一 provenance 门禁；body `{provider,tool,arguments}`，执行已授权操作；HTTP request drop 取消本次调用的 token。 |

无 principal 为 HTTP 401，非 LocalOperator（包括 PublicTunnel）为 HTTP 403。**PublicTunnel 不能调用这四条接口（前三条及本轮 preview），连宿主授权候选读取也不能**；不能因为其他 MCP 配置接口支持公开隧道只读，就把这四条误写成公开隧道可读。桌面通过 Tauri，本地 shared-live 和 standalone 通过 HTTP 复用同一服务；`WebHostServices` 注入缺失时返回 `UNAVAILABLE`，不是成功的空列表。接线来源另见 `src-tauri/src/web/mod.rs:514-516`、`src-tauri/src/server_main.rs:525,603`、`src-tauri/src/lib.rs:3638-3652` 与 `src-tauri/src/remote/host.rs`。

#### operation-specific scopes（由 se.mcp.hostCall 覆盖，不重复计数）

| provider / tools | class and scope | host-enforced boundary / caveat |
|---|---|---|
| workspace.group_list / project_list / project_get | R；授权 project 集合及其 group metadata | 全局 policy 和 workspace.enabled 均默认关闭；`projectIds:null` 为显式授权全部，`[]` 为零项目，集合只可见其成员；`includePaths` 独立控制路径投影。分页和分组计数必须先过滤授权项目，不能泄漏未授权成员、preferredProjectId 或总数。registry 未同步返回 `REGISTRY_NOT_READY`，不伪装成真实空集；group/project list 使用 cursor/limit。 |
| ssh.profile_list / profile_get | R；显式授权 sshProfile | 仅脱敏资料与已授权能力；不读取或回传凭据值，不能把管理接口的全部候选视图误当作此工具输出。profile_list 为 offset/limit 分页。 |
| ssh.test_connection | W（主动联网）；sshProfile + operation | 仅使用已信任的 host key，不自动登记/改变 host trust；不是仅凭 profile 可见即可测试。 |
| ssh.execute / job_start | W/D；sshProfile + execute 或 job_start operation | 显式执行授权等于远端账号权限，不承诺 remoteRoots 能沙箱任意 shell；job_start 只需 job_start 操作授权（posixJobs 字段保留兼容但不再作为额外闸门），远端 POSIX shell 能力由宿主判定。勾选的操作即完整授权；remoteRoots/localRoots 为可选路径限制，留空表示不限路径，填写后才收窄。execute 的 timeoutMs、输出上限均受约束；job_start.runtimeLimit 单位是**毫秒**，不误写成秒。不自动重试执行。  MCP-only 全局/profile commandDeny 并集在凭据/网络/worker/job 副作用前强制检查；直接 MCP managed-job 入口与分发复用同一宿主门禁，非法规则 fail closed；不限制普通终端/PTY/交互 SSH。 |
| ssh.list_files / read_file | R；sshProfile + operation + remoteRoots | 必须 host-side 校验规范化路径/远端 roots；有限分页、字节偏移与输出上限；读取到的文件内容为不可信数据，不能宣称远端文件天然不含用户敏感内容。list_files 使用 offset/limit，而不是 workspace cursor。 |
| ssh.write_file | W/D；sshProfile + operation + remoteRoots | 仅创建新 UTF-8 文件；当前不支持覆盖已有文件（overwrite 只允许 false），不把旧 SFTP 的任意覆盖契约套过来。 |
| ssh.upload / download | W/D；sshProfile + operation + remoteRoots + localRoots | localPath 是 **Se 宿主文件系统**，不是外部 MCP 客户端机器；上传为本地读+远端写，下载为远端读+本地写，不能标成 read-only。单文件、非递归，当前 overwrite 只允许 false。 |
| ssh.job_status / job_list / job_cancel | R / R / W/D；caller + profileId + jobId（取消逐次确认） | 每个 job_* 都需要 profileId；status/cancel 另需 jobId，按宿主归属与授权 profile 过滤，不借 job id 绕过 scope。job_list 用 offset/limit，job_status 用字节 offset/maxBytes。cancel_requested 或 channel/future drop 不等于 remote termination confirmed；不得以断开用户 SSH 终端代替取消。 |

公共 schema 以 `src-tauri/src/mcp_core/host_tools.rs:58-155` 为准；annotation 不是授权门禁。具体 profile grant、operations、localRoots/remoteRoots 和 job 所属检查由宿主服务执行，插件不能通过 tool arguments 扩权。原始 MCP 使用统一主体 `mcp-global`；**所有连接到本宿主的 MCP 客户端共享这套全局授权**，目前没有按客户端/插件的多租户隔离，不能把 catalogue 的目标 caller 范围写成当前实现已经提供的身份隔离。

#### 私有 loopback bridge：INTERNAL，不向插件发布

| internal surface | source | exposure | covering op / boundary |
|---|---|---|---|
| POST /v1/call | `src-tauri/src/mcp_core/host_bridge.rs:249`；路由 `:211-215` | INTERNAL | gateway/core 到宿主 `se.mcp.hostCall` 的私有传输，不是另一个公网或插件入口。独立 bearer token 鉴权，监听 `127.0.0.1:0`，descriptor 用户私有（0600，并校验权限/大小）；token 与 descriptor 不通过任何 OPEN/OPEN-REDACTED op、事件、日志返回插件。 |
| POST /v1/cancel | `src-tauri/src/mcp_core/host_bridge.rs:171`；路由 `:211-215` | INTERNAL | 同一私有 loopback/token 信任边界内，按 requestId（wire 字段 `request_id`）传播显式 cancel 或 future drop；bounded cancellation ledger/tombstone 处理 cancel-before-call 与有界回收。不计新插件 op，不给插件私有 token，也不把 best-effort cancellation 说成远端进程已确认停止。 |

bridge 的独立 token 不是外部 MCP bearer，也不是插件授权令牌。HTTP request drop 与 bridge future drop 的取消传播已经存在；桌面 `mcp_host_call` 当前仅创建 `CancellationToken::new()`，没有同样的 IPC request-drop/cancel 通道，必须保留此 transport 差异。SSH `job_cancel` 仍是公开通用调用中受授权的业务操作，不等于把内部 `/v1/cancel` 暴露给插件。

#### renderer / 持久化状态的覆盖（复用已有 op，不增加前端独有计数）

- **workspace groups**：`src/renderer/hooks/use-projects-persistence.ts:250-278,469-478,738-760` 将 groups 与 project snapshot 一起持久化，并通过 `src/renderer/lib/tauri-remote-api.ts:111-119` → 既有 `remote_sync_projects`（`src-tauri/src/commands.rs:5035-5057`）把 projects/groups/defaultProjectId 同步给宿主 `ProjectRegistry::set_with_groups`。分别由既有 `projects.groups.add/rename/remove/reorder`、`projects.move_to_group/reorder` 与 `se.remote.syncProjects` 覆盖；新增 group summaries、成员顺序及 preferred/default 字段必须做 allowlist 投影，不开放原始 persistence store。activeGroupId 留在客户端本地，不能声称是宿主全局激活组；后端只读查询由 `se.mcp.hostCall` 的 workspace 三工具覆盖，不赋予 renderer 写状态能力。
- **宿主授权面板状态**：`src/renderer/components/settings/McpHostAccessPanel.tsx:24-401`、`src/renderer/lib/mcp-host-api.ts:13-61`、`src/shared/types/mcp-host.types.ts:25-73`，由 `McpControlPanel.tsx` 挂载。候选/策略加载覆盖 `se.mcp.hostAccessGet`，保存全局 enabled/workspace 范围/includePaths/SSH profiles/operations/roots/posixJobs/fingerprints 覆盖 `se.mcp.hostAccessPut`，测试连接、任务查询/取消覆盖 `se.mcp.hostCall`；本轮全局/profile 黑名单编辑仍由 get/put 覆盖，普通授权编辑须保留两层规则；纯管理草稿预览由 `se.mcp.hostAccessPreview` 覆盖，测试 command/草稿只在内存，不存 localStorage/URL/analytics，未保存表单草稿不另作能力。SSH job UI 需按 operation grant 门禁，PublicTunnel 面板锁定不代替后端鉴权。宿主策略保存于 app-data `mcp-host-policy.json`，不是 renderer 私有持久化授权，也不是凭据文件。
- **事件**：本次未新增独立宿主授权变更或 job progress 插件事件，不发明 `mcp.hostAccessChanged`、SSH job completion 等 topic。组同步可复用源码已有 `projects_changed`，不重复新增事件计数；现有清单 C 的 renderer event model 仍是 proposed contract，而不是已打通的事实。任务进度以 job_status/job_list 查询为准。

#### 凭据 backend 更正与插件适配边界

`src-tauri/src/credentials.rs:85-93,154-175,213-239` 明确：当前 shipped backend 是用户私有 JSON 文件（通常 `~/.se-manager/credentials.json`，品牌目录/`SE_PROJECT_ROOT` 参与解析），写入用 0600 和原子替换，GUI、cores 和 gateway 共用。**不是 OS keychain，也不能仅凭 0600 声称内容已加密。** `secure_storage.rs:85-139` 的 `keyring_get/keyring_set` 与 legacy/canonical service 名是兼容 API/命名空间，实际调用 `credentials::backend()`；SSH 的 `credential_store` 也走同一 seam。清单中的 SSH、secure_storage、OAuth/tunnel 凭据表述以此更正为准；“不存进 profile JSON”不等于“不存 JSON”。

宿主复用 SSH 凭据是 OPEN 业务操作，凭据值的直接读写仍按原有 SECRET-EXCLUDED 契约，不把整个 SSH feature 排除；公共管理视图不含密码、passphrase、私钥内容、bearer/私有 bridge token。远端命令输出、文件、日志本身是不可信/可能敏感的业务内容，不能将“宿主不返回凭据存储值”夸大为“任意用户命令输出均无秘密”。

已检查当前没有 `src-tauri/src/capability_host/` 运行时插件 operation registry；现有 `BuiltInRegistry` 是 MCP 内置工具注册器，不等同于插件平台 C1–C3 的 descriptor/provider。此变更仅同步目录；建议运行时插件适配就绪时补充四个 descriptor（前轮三个及本轮 preview）、按 op-specific scope 的 provider 投影、per-call 确认/审计、越权和 sentinel 测试，**不在本次文档 scope 内修改产品代码或伪称这些已实现**。

### SSH 命令黑名单 v2 与纯管理预览（2026-11-07）

本轮新增 `se.mcp.hostAccessPreview`，分类为 **R / global（当前 sshProfile 的管理预览）/ none / OPEN / rust-main**；不是命令执行能力，不新增 MCP provider 子工具。以下为本轮目录契约；transport 注册已只读核对，核心/UI 行为由对应实现与回归验收，不以本次目录检查冒充整体功能验收。

- **v2 策略字段：** `mcp_host_access_get` 返回、`mcp_host_access_put` CAS 保存的 `HostAccessPolicy` 包含 `ssh.commandDeny` 与 `ssh.profiles[profileId].commandDeny` 数组；规则为 `{id,kind:"exact"或"contains"或"regex",pattern}`。v1 缺失字段兼容为空数组，不能因迁移或普通授权编辑隐式扩大授权、丢失规则或重置身份 fingerprints。全局规则与当前 profile 规则取并集，profile 不可覆盖/清除全局 deny；新增授权默认全部 13 个 SSH operations 也不能绕过 deny。非法规则必须整份拒绝、fail closed，不能忽略坏规则继续执行。
- **共享宿主门禁：** `ssh.execute`、`ssh.job_start`、MCP 专用 managed-job 的直接 JobManager start 及 worker 分发均复用宿主权威规则；初始拒绝须早于凭据解析、网络、worker 入队、job slot/记录/持久化。排队及实际分发前复查权威策略；预览、前端判断或调用者参数不能成为执行许可。job_status/job_list/job_cancel 仍独立授权，不拿内部管理包装命令匹配黑名单，也不承诺追杀已运行任务。
- **匹配与边界：** exact 对原始 command 整串相等、contains 为字面子串、regex 使用 Rust regex search；不隐式 trim/重写输入。文本 deny 只是防误操作护栏，不是 shell 沙箱，不能保证阻止脚本/变量/解释器/编码等语义绕过，也不控制普通本地终端、PTY/terminal-core、交互 SSH shell 或其他 SSH 客户端；这些终端路径及共享底层连接不读取/追加该 MCP-only policy。是否限制按 MCP 工具调用链判断，从终端启动 MCP 客户端仍受限。文件与 SFTP 操作继续按各自 roots/operation 授权，不能宣称它们自动受到命令黑名单控制。
- **纯管理 preview：** `mcp_host_access_preview(request)` 输入 `{profileId,operation:"execute"或"job_start",command,policy?:HostAccessPolicy}`，policy 是可选未保存草稿；输出 `{outcome:"matched"或"not_matched",matches:[{id,source:"global"或"profile"}],policyRevision,draft,blockedBy?}`。返回匹配 ID/来源，不回显原始 command、pattern 或匹配片段；非法规则经 IPC 错误返回，不伪装成 `not_matched`。未命中不等于可执行，仍需展示 blockedBy；draft 不写盘、不授予权限、不替换权威策略。
- **无执行副作用与隐私：** preview 不读取 credentials、不联网、不取得 worker permit/排队、不创建 jobs、不持久化；允许读取必要的策略/profile 管理元数据，不宣称零磁盘读取。测试文本/草稿只在内存，不进 URL、localStorage 或 analytics；日志/审计仅稳定操作、结果码、revision 等元数据，不记 command、pattern、匹配片段或完整异常。规则字段本身不是凭据值，但管理策略/规则文本可能敏感，仍需按调用方 scope 最小化投影，不能把管理员完整 policy 公开给无权限插件。
- **transport 已接线：** IPC `src-tauri/src/commands.rs:5679-5693`、注册 `src-tauri/src/lib.rs:4066`；HTTP `POST /mcp/host/access/preview` 对应 `src-tauri/src/web/mcp_host_api.rs:120-140`，`src-tauri/src/web/router.rs:83,95` 在两种 provenance 分支均注册（请求 body 上限 1 MiB），但 handler 仍 `require_local`：无 principal 401，PublicTunnel/非 LocalOperator 403，缺服务返回 `UNAVAILABLE`。HTTP twin 不另计能力；桌面、shared-live、standalone 与 browser 复用同一 preview 服务，不提供 public tunnel 只读豁免。

### Remote desktop host (P0/P1/P2)

| op id | source cmd (file:line) | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.remote.desktop.getSettings | get_remote_desktop_settings — remote/desktop.rs | R | profile | none | OPEN | Local operator only; `{supported,viewEnabled,controlEnabled,permissions:{screenRecording,accessibility}}`; view defaults on, control off. Browser facade returns unsupported; Linux host supported:false. No secrets. |
| se.remote.desktop.setSettings | set_remote_desktop_settings — remote/desktop.rs | W | profile | grant | OPEN | Local operator only; `(viewEnabled,controlEnabled)` returns complete settings. Paired devices cannot grant themselves control. Turning control on rearms native input; off invalidates controllers. |
| se.remote.desktop.emergencyStop | remote_desktop_emergency_stop — remote/desktop.rs | W | profile | none | OPEN | Safety action requires no confirmation: kill input, close every desktop stream, persist control=false. In-app ⌃⌥⌘. fallback only (no global-shortcut dependency). |

| op id | source route | class | scope | confirm | exposure | notes |
|---|---|---|---|---|---|---|
| se.remote.desktop.info | GET /remote/desktop/info — web/router.rs; web/remote_desktop.rs | R | profile | none | OPEN | RemoteDesktop classification, screen.view plus viewEnabled. Returns settings/permissions plus displays and `codecs`; Linux supported:false and displays:[]. `codecs` is `["jpeg"]`, or `["h264","jpeg"]` on macOS when a VideoToolbox session can be created (probed once, cached). LAN and tunnel share policy. |
| se.remote.desktop.screenshot | GET /remote/desktop/screenshot — web/router.rs; web/remote_desktop.rs | R | profile | none | OPEN | RemoteDesktop; screen.view plus viewEnabled; query display, format=jpeg/png, quality. Image bytes, no-store and MIME; pixels never logged. |
| se.remote.desktop.stream | GET /remote/desktop/stream — web/router.rs; web/remote_desktop.rs | W | profile | per-call | OPEN | RemoteDesktop WS upgrade; Origin and bearer/first-frame auth match terminal WS. Viewing needs screen.view plus viewEnabled; each input needs screen.control plus desktop-local controlEnabled. Stream begin is caller initiation; local consent remains mandatory. Only revalidated DEVICE_REVOKED closes 1008; desktop-only stop/disable/scope failures send stable error with capability then close 1000/1001 without revoking device. Terminal errors bypass queued video before Close; bounded try_send drops video and requests keyframe. Start accepts `codec` (`h264`|`jpeg`, default `jpeg`) and `bitrate` kbps (500..20000, default 4000); fps is 1..5 for jpeg, 1..30 for h264. meta always reports the negotiated `codec`; unsupported h264 falls back to jpeg and is logged. H.264 frames use the same `[u32 header][JSON][payload]` envelope with `type:"h264"` and keyframe SPS/PPS. |
| se.remote.desktop.webGetSettings | GET /remote-desktop/settings; GET /api/remote-desktop/settings — web/router.rs | R | profile | none | OPEN | LocalOperator only; facade parity with get_remote_desktop_settings; paired tokens/tunnel cannot operate consent. |
| se.remote.desktop.webSetSettings | PUT /remote-desktop/settings; PUT /api/remote-desktop/settings — web/router.rs | W | profile | grant | OPEN | LocalOperator only; full camelCase boolean update; same persistence as Tauri. |
| se.remote.desktop.webEmergencyStop | POST /remote-desktop/stop; POST /api/remote-desktop/stop — web/router.rs | W | profile | none | OPEN | LocalOperator safety stop, returns `{ok:true}`. Closes viewers/controllers and disables control. |

| event id | source emit (file:line) | payload summary | rate | scope |
|---|---|---|---|---|
| remote-desktop://state | remote/desktop.rs | `{viewers,controllers,display,controlEnabled}`; counts and selected display identity only, never pixels/input/text | activity changes and active-session replay | profile |

## 不变量

1. **密钥不出宿主。** 9 个 SECRET-EXCLUDED 操作在注册表中不存在。即使用户授权，也没有任何 op、事件、日志或审计条目能返回密钥值。OPEN-REDACTED 的输出由契约测试用哨兵值验证不含密钥。
2. **身份与范围由宿主派生，不信任插件输入。** caller、pluginId 和 project 范围从已认证连接得出。路径、cwd、terminalId 和 conversationId 先在宿主侧解析，并校验是否落在已授权范围内。清单发现以下现有缺口，开放前必须补上：
   - `terminal_watch`、`terminal_write` 没有 claim 校验，插件版本必须校验调用方是否拥有该终端（`commands.rs:1244,1486`）；
   - `git_*` 的读操作和 `push` 不校验项目路径，插件版本必须先把 cwd 规范化并核对它在已授权项目之内（`commands.rs:6767` 等）；
   - `sftp_download` 等 4 组操作接受任意本地绝对路径，必须限定在授权根目录内或每次确认；
   - `mcp_connection_info` 返回网关 bearer token，`mcp_get/put_config` 和 `remote_*_mcp_registry` 带 env/header，`remote_server_rotate_credential` 返回新 token，以上都必须去掉密钥后再开放；
   - `RemoteStatus` 的字段和 `plugin_web_read` 的路径限定尚未逐项核实，核实前按 OPEN-REDACTED 和仅限插件包目录处理。
3. **危险操作按 `per-call` 每次确认，不能由安装时授权替代。** 典型的有：`se.sessions.respondPermission`、`se.terminal.write`、`se.git.push/discard`、删除类操作、安装原生代码或 agent。没有本机确认者时一律返回 `CAPABILITY_CONFIRMATION_REQUIRED`。
4. **只读不改状态。** 标为 R 的操作不得触发迁移、索引构建或写盘。现有的 `memory_index` 读路径会顺带执行迁移，开放前要按平台方案 Design G 改成只读打开。
5. **长任务异步执行。** 构建索引、clone、安装、运行定时任务、发提示这类操作立即返回 jobId，再通过事件和状态查询推进，不占用插件 2 秒的回调时限。缺少取消入口的要补齐：未注册的 `memory_index_cancel_cmd`，以及没有取消命令的 SFTP 上传和下载。
6. **每次调用都记审计，只记元数据。** 时间、调用方、op、范围、结果码和确认来源都要记录，不记录参数和返回内容。

7. **宿主 MCP 授权默认关闭且全局共享。** 调用同时校验宿主 policy、workspace 或 SSH operation scopes；管理策略是高权限委托，插件必须逐次本机确认，不能依靠安装 grant 或 caller-supplied grant/confirmed 自授权。现有 `mcp-global` 不是多租户隔离。
8. **私有 bridge 永不成为插件凭据获取接口。** `/v1/call` 与 `/v1/cancel` 均 INTERNAL；descriptor/token 留在用户私有 loopback 信任边界。取消只传播请求意图，除非有真实远端证据，不能报告 termination confirmed；Tauri/web/gateway 的取消能力差异必须如实呈现。
9. **脱敏不等于 metadata 无风险。** `hostAccessGet` 的候选 profile、project id/name、policy roots/路径仍按调用者 scope 最小化；原始本地管理员视图不可直接当作无权限插件 DTO。凭据 backend 是用户私有文件，不再称作 OS keychain。

10. **命令 deny 不可降级成前端提示。** 全局与 profile 规则只叠加，execute/job_start 和直接 job 入口共享宿主 fail-closed 门禁；默认 13 操作授权、草稿预览和 `not_matched` 均不能越过它。纯管理 preview 不执行、不保存、不返回原始 command/pattern，不改变普通 PTY 或远端 shell 的安全模型。

## 验收

1. **覆盖完整：** 自动比对 `src-tauri/src/lib.rs` 的 `generate_handler!` 和注册表。每个命令必须恰好对应一个 OPEN、OPEN-REDACTED、SECRET-EXCLUDED 或 INTERNAL 条目，新增命令没有分类时 CI 失败。
2. **密钥不泄露：** 在凭据存储、MCP 配置、远程访问凭据和 AI 渠道里放入哨兵值，调用全部 OPEN 和 OPEN-REDACTED 操作，并订阅全部事件。断言返回值、事件、审计和日志都不含哨兵值。
3. **范围越权被拒：** 对每个带路径或带 id 的操作，分别测试越权项目、越权终端、越权会话和项目外路径，断言被拒绝且没有副作用。
4. **确认生效：** 不经确认调用 `per-call` 操作时，断言返回 `CAPABILITY_CONFIRMATION_REQUIRED` 且没有副作用；确认令牌只能使用一次。
5. **验收场景 H1：** 第三方插件「整理历史会话」完整跑通（见 `plugin-platform-plan` 的 H1）。
6. **核实遗留项：** 以下各项要逐项核实，结论写回本清单：`conversation_migration_control` 的参数、`acp_fetch_registry_snapshot` 和 `acp_install_registry_binary` 的实现、`RemoteStatus` 的字段、`plugin_web_read` 的路径限定、通知 store、命令面板的置顶与最近使用、项目默认 shell 的写入路径。

7. **本次目录同步检查：** `python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py` 应输出 `registered commands: 329; unclassified: 0`；合并统计为后端 329（OPEN 292 / OPEN-REDACTED 12 / SECRET-EXCLUDED 8 / INTERNAL 17）与前端 38，共 367；原宿主 MCP 追加后的后端 328 / OPEN 291、总计 366 / OPEN 329 为本轮前基线，仅新增 preview 一个 OPEN。确认既有来源、清单及事件完整保留，新增命令各仅一个主分类；四条 HTTP twin（前轮三条及本轮 preview）、两条 INTERNAL bridge 路由和 provider 子工具不重复计数。历史 explorer 子表继续按原约定保留。
8. **后续插件运行时验收（本次不宣称通过）：** 四个增量 op 的成功/越权/哨兵不泄漏测试（其中 put/call 的无确认 per-call 拒绝；get/preview 为 none）；所有 workspace group 计数先过滤授权成员，未同步区别于空集；SSH profile+operation+roots+job 归属限制；HTTP 无 principal 为 401、PublicTunnel 为 403、缺服务为 UNAVAILABLE；私有 bridge token 不出插件边界，取消不误报远端停止。检查策略或相关表单改动不能隐式扩大授权；按插件身份隔离若未实现必须明确标注，不得以共享 MCP policy 冒充多租户授权。
9. **本轮 SSH deny 目录契约（运行时由对应 owner 验收，本次仅静态同步）：** 覆盖 v2 规则读写、全局/profile 并集、非法规则 fail closed、默认全部 13 操作仍受 deny、execute/job_start/direct-job/worker 统一门禁及普通授权编辑保留规则；preview 的 matched/not_matched、matches ID/source、policyRevision/draft/blockedBy 与非法规则 IPC 错误均有对应实现验收。验证 preview 零 credentials/network/worker/jobs/persist、无 command/pattern 回显及日志泄漏；两种 HTTP provenance 分支均注册但 PublicTunnel 必须 403。不得把目录 checker 通过写成 Rust/UI 功能回归或插件 registry 已实现。
