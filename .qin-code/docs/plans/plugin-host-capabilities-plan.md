---
schema: qin-doc/1
kind: plan
id: plugin-host-capabilities
title: 原生插件宿主能力第一批：上下文/事件、存储、反馈、菜单快捷键、状态栏
date: 2026-11-03
status: accepted
source: user:2026-11-03 "开始新增这部分能力 然后分析还应该增加哪些能力"; agent://plugin-capability-research; agent://cap-planner
lang: zh-CN
scope: project
---

## 目标

在 ABI v1 不变、Plugin Core 独立进程、声明式 UiTree 的前提下，为原生插件交付第一批宿主能力。完整设计（契约、限额、数据流、file:line 锚点）见规划结果 `agent://cap-planner`，本文件是其可执行摘要。侧边栏入口已在 `690a0b48` 交付。

1. **上下文 + 事件** `se.host.context` / `se.host.events`：仅主窗口。五个 topic：`project.activeChanged`、`terminal.created`、`terminal.closed`（从受管终端注册表移除，不是隐藏标签页）、`terminal.activeChanged`、`agent.session.stateChanged`（真实四态 initializing/active/error/closed + activeTurn，不虚构 completed）。manifest `contributes.eventSubscriptions` 声明订阅，宿主经现有 `on_service_call`（method `deliver`）投递；每个订阅有 sequence，溢出/重置发 `resyncRequired` 标记，插件取 snapshot 后丢弃水位以下事件。
2. **存储** `se.host.storage`：profile / project 两级 KV，所有操作返回 ticket，完成经 `completed` 回调；namespace revision CAS，新错误码 `PLUGIN_STORAGE_CONFLICT` / `PLUGIN_QUOTA_EXCEEDED`；单文件 `<profile>/plugins/data/<pluginId>.json`，1 MiB / 256 key；publication 即提交点；禁用/升级/卸载均保留数据。
3. **反馈** `se.host.feedback`：`notification.upsert/close`、`progress.upsert`，由宿主自有 Sonner 卡片渲染，带插件署名；动作只能引用已声明命令。
4. **菜单 + 快捷键** `contributes.menus`（`terminal.tab`、`terminal.context`、`project.context`、`agentSession.context`）与 `contributes.keybindings`（接入现有 keyboard-shortcuts store，ctrl→cmd 复用现有匹配，内置优先，冲突默认不绑定，用户自定义跨禁用/重载/方案切换保留）；`when` 仅允许 `project.exists`、`terminal.exists`、`agentSession.exists`、`textInput.focused`、`terminal.focused` 五个布尔键的 AND。
5. **状态栏** `contributes.statusItems`（每插件 ≤2）+ `se.host.status.set/clear`，渲染在现有 StatusBar 右侧，图标复用 sidebar 图标白名单。

公共约束：

- 新 host 服务回复统一 `{"schemaVersion":1,"result":{"ok":true,"value":…}}` / `{"ok":false,"error":{code,message}}` 信封，避免业务错误码在 ABI 状态码转换中丢失。
- 所有 `se.host.*` 名称保留，插件不得 provide。broker 在授权后、provider 查找前分流到独立 `HostServices`；它从不重入调用方 worker、不持 runtime 状态锁、不在回调线程做磁盘 IO、不等待 GUI。
- 新增内部 `CallKind::HostDelivery` 复用 `on_service_call`，每插件同时至多一个投递，回调返回后才处理 `invalidateViews`。
- 所有临时资源（订阅、ticket、通知、进度、状态、渲染注册）按 `(bootId, pluginId, generation)` 租约，禁用/升级/超时/Core 断开即撤销。
- 菜单、快捷键、状态项、通知动作统一走 `plugins.contribution.execute`，由 Core 从 manifest 解析命令并校验 generation 与目标。
- 握手新增能力 `plugins.host-capabilities.v1`。

## 非目标

- `se.host.secrets`：**推迟**。生产凭据后端是明文 JSON（`src-tauri/src/credentials.rs:85`），`keyring` 仅为 dev 依赖；保留服务名但报告 unavailable（`PLUGIN_NOT_SUPPORTED`），绝不回落到明文。
- 不改 ABI vtable，不做 webview / HTML / 插件 CSS，不做 TS/JS 插件，不做 OS 通知或通知中心（状态栏铃铛保持原样）。
- 不提供终端输出、按键、cwd、git、MCP、workspace 事件；不支持远程/浏览器客户端（返回 `PLUGIN_NOT_SUPPORTED`，无副作用）。
- 不承诺事件 exactly-once 或重启重放；能力声明是 API 契约，不是沙箱。

## 任务

| 包 | 独占文件 | 依赖 |
|---|---|---|
| P1 共享契约 | `se-plugin-protocol/**`、`src/shared/types/plugin.types.ts(+test)`、新 `src/shared/types/plugin-host.types.ts`、`src/shared/plugin-fixtures/contracts.json` | sidebar（690a0b48） |
| P2 宿主服务 | `se-plugin-host/src/**`（新 `host_services.rs`、`host_context.rs`、`storage.rs`、`host_feedback.rs`）；公开 `publish_host_context`、`heartbeat_host_context`、`detach_host_context`、`host_ui_snapshot`、`dismiss_host_ui`、`execute_contribution` | P1 |
| P3 GUI/Core 传输 | `src-tauri/src/core/plugin.rs`、`src-tauri/src/plugin_commands.rs`、`src-tauri/src/lib.rs`、`se-terminal-core/src/ipc.rs`；RPC `plugins.host.context.publish/heartbeat`、`plugins.host.ui.get/dismiss`、`plugins.contribution.execute`，事件 `host_ui_changed`；发布仅限 `main` 窗口 | P1、P2 公开方法 |
| P4 渲染层 | `features/plugins/` 新 host-* 组件与 store、`plugin-api.ts`、`PortableAppEffects.tsx`、`StatusBar.tsx`、`tab-context-menu.tsx`、`WorkspaceTabBar.tsx`、`ConnectedTerminal.tsx`、`ProjectSidebar.tsx`、`keyboard-shortcuts-store.ts`、`use-keyboard-shortcuts.ts`、`WorkspaceLayout.tsx`、`AppPreferences.tsx`、plugins 语言包 | P1（P3 前用假传输） |
| P5 试点 + 集成 | `se-plugin-example/**`、`se-plugin-host/tests/plugin_e2e.rs`、`src-tauri/tests/plugin_core_smoke.rs`、`scripts/test-plugins.sh`、`features/plugins/activity-pilot.integration.test.tsx` | P2、P3、P4 |

执行顺序：P1 → 根验证提交 → P2 ∥ P4 → P3 → P5 → 根逐包验证并按 pathspec 提交。执行者不提交。

试点：计数器插件新增 Activity 区块，观察 agent 回合由 activeTurn true→false 时计数（CAS 写入 project KV `observedTurns`）、弹出「Agent turn ended」通知、进度卡原地更新、状态栏 `Agent active` / `Agent idle`；四个菜单位置 + 一个默认未绑定的快捷键执行 `example.activity-summary`。test-hooks 构建额外提供 `example.activity-inspect` 与投递 panic/超时开关。

## 验收

- P1：`cargo test -p se-plugin-abi -p se-plugin-protocol`，`bunx vitest run src/shared/types/plugin.types.test.ts`；共享 fixture 在 Rust/TS 分类一致，旧 manifest 与 sidebar fixture 仍有效。
- P2：`cargo test -p se-plugin-host --lib`，含 `reserved_host_services_never_reenter_caller`、`host_requirements_resolve_without_plugin_providers`、`host_delivery_waits_for_callback_return`、`event_overflow_requires_snapshot_watermark`、`project_switch_resets_scoped_subscriptions`、`withdraw_revokes_pending_host_resources`、`storage_namespace_cas_and_quotas`、`storage_publication_is_commit_point`、`host_actions_reject_stale_generation_and_targets`、`secrets_dependency_has_no_plaintext_fallback`。
- P3：`cargo test -p se-manager --lib plugin`、`-p se-terminal-core --lib ipc::tests`；非主窗口不能发布上下文，断线 owner 不能清除替代源，`host_ui_changed` 跨重连可恢复。
- P4：相关 vitest + `typecheck:web` / `typecheck:test`；通知动作、进度原地更新、状态项、四处菜单、快捷键 ctrl→cmd 与冲突优先级、浏览器零副作用、隐藏终端标签不发 `terminal.closed`。
- P5：`bash scripts/test-plugins.sh` 与试点集成测试通过；Terminal/ACP Core 在 Plugin Core 故障时不受影响。
- 全局：插件 crate 严格 clippy、tsc ×3、完整 vitest 对比基线无新增失败；reviewer 审查；隔离实例（`com.se-manager.app.plugintest`）用 cua-driver 实测侧栏、通知、状态栏、菜单、快捷键，并证明用户 Se 进程未受影响。
- 所有 cargo 命令带 `--target-dir /Users/qs/project/me/termul/src-tauri/target`。
