---
schema: qin-reference/1
id: herdr-agent-worktree
title: herdr agent 状态控制、事件订阅与 worktree 隔离
kind: external-project
source: agent
date: 2026-10-23
status: inbox
license: Apache-2.0（Cargo.toml 与 LICENSE，固定 revision 5bd900d88c80d95f5b145397e45bbeea398b8165）
tags:
- agent-state
- socket-api
- events
- wait
- subscriptions
- worktree
- cli
- se-manager-comparison
links:
- url:https://github.com/herdrdev/herdr
- url:https://github.com/herdrdev/herdr/tree/5bd900d88c80d95f5b145397e45bbeea398b8165
---

## 摘要
研究对象：herdrdev/herdr，固定 revision `5bd900d88c80d95f5b145397e45bbeea398b8165`（Cargo version 0.9.5），观察日期 2026-10-23。Canonical URL: https://github.com/herdrdev/herdr 。Cargo.toml 宣称 Apache-2.0、版本 0.9.5、仓库地址（固定提交源码证据：`Cargo.toml:1-9`）。公开 GitHub 搜索结果显示约 44.0k stars、1,861 commits；stars 为观察时上下文，不作质量评价。项目高层文档将 socket API 定位为脚本/工具/agent 控制面，并推荐 CLI wrappers，raw socket 用于直接请求响应和长连接订阅（固定提交 `docs/versions/0.9.5/website/src/content/docs/socket-api.mdx:6-18`）。

## 可借鉴
1. **状态检测防抖与生命周期发布（borrow/adapt，价值高、改造中）**：`src/pane/agent_detection.rs:5-13` 设定 100ms 重检、3 次确认、700ms 上限、800ms blocker 刷新、3s 启动宽限；`39-77` 仅在 working→无可见信号 idle 时暂存，显式 idle/blocker、进程退出、agent 变化会旁路；`184-212` 只有状态/可见信号/agent变化/退出或 blocker 定时刷新才发布。代码还能在 `342-360` 将进程退出映射为显式 idle，并跳过 `skip_state_update`。这比本地 `src/renderer/lib/agents/agent-terminal-state.ts:42-53,74-81` 的证据派生和 `src/renderer/hooks/use-agent-terminal-state.ts:25-36,51-72,94-125` 的 600ms 输出门控更接近稳定的 host-side 状态机；本地已有 unknown 与 settle pass，但没有 herdr 的 pending-idle confirmation / process-exit publish / stable blocker refresh。
2. **Agent CLI/socket 的合并 schema、身份固定 wait 与原子 prompt+wait（borrow，价值高、改造中）**：参数结构在 `src/api/schema/agents.rs:9-68,184-206`；AgentInfo 同时携带状态、序列、workspace/tab/pane、cwd/foreground_cwd（`209-252`），可让控制端诊断“状态来自哪个 pane/会话”。`src/api/wait.rs:132-174` 先读取当前状态，命中即返回，否则从 event sequence 继续等待；`537-559` 以 terminal_id、name、agent 和 state_change_seq 做身份/新变化约束；`650-695` 区分 timeout、agent_prompt_stalled、agent_not_running。文档明确 `agent.wait` server-owned、固定 resolved pane occupant；`agent.prompt` 可携带 wait 避免分离调用竞态，已有 blocked 时不发送输入（固定提交 `socket-api.mdx:114-115`）。本地 AgentLauncher 已把 worktree cwd 先建好再创建会话（`src/renderer/features/agent-session/agents/AgentLauncher.tsx:1118-1147,1210-1218`），但状态控制目前以 renderer 规则/轮询为主，不是服务端事件驱动。
3. **事件协议可验证、订阅有快照/历史游标/丢失恢复契约（adapt，价值高、改造中）**：事件 schema 中将 `pane.agent_detected` 与 `pane.agent_status_changed` 分开（`src/api/schema/events.rs:17-85,176-220`）；AgentStatus subscription 初始 probe 读取当前 PaneInfo，记录 last status/presentation/sequence，并在当前状态已经匹配时生成 initial event（`src/api/subscriptions.rs:206-236`），历史事件后再 snapshot poll（`250-322,397-419`）。历史不足返回 `events_lost`，要求 resubscribe + session.snapshot（`326-339`）；socket 集成测试验证 128 事件批量无每事件轮询延迟、历史丢失先报错、单个慢连接不影响健康连接（`src/api/server/subscription_socket_tests.rs:259-356`）。本地没有一个等价的 agent 状态 wire protocol；`terminal-store` 状态字段主要内存态（本地类型证据 `src/renderer/types/project.ts:168-185`），远程/web/iOS parity 要由共享 Rust/API 补齐。
4. **worktree 与 workspace membership 分离、父 repo 作为操作源（borrow/adapt，价值高、改造中）**：`src/app/api/worktrees.rs:28-38` 要求绝对路径；`167-210` 从 workspace_id 或 cwd 解析源，拒绝 linked worktree 作为新建源（`197-200`）；`finish_worktree_open` 校验 git metadata/repo key/checkout key，避免 discovery 后路径被替换（`69-90`）；创建异步化且以 checkout key 去重、禁止同一路径并发操作（固定提交 `src/app/api/worktrees/deferred.rs:100-167`），完成事件统一回收 pending 状态（`227-243`）。测试验证创建后 workspace 数、linked_worktree 标识、事件顺序和插件上下文（`src/app/api/worktrees.rs:760-845`，`848-924`）。本地 worktree API 已有 `projectPath/name/branch/startRef/targetPath` 与 web/remote 分流（`src/renderer/lib/worktree-api.ts:65-123`），AgentLauncher 先创建 worktree 再把 launchCwd 切换到它（`AgentLauncher.tsx:1118-1218`）；但当前可见证据把 worktree 绑定在 project/terminal/chat target 上，没有 herdr 那样的 workspace membership + operation-key/async completion 协议。

## 不借鉴
- **不直接复制检测字符串或代码**：本地明确说明规则只是对第三方 CLI 可观察输出的重新表达，不复制 herdr 源码（`src/renderer/lib/agents/agent-terminal-state.ts:15-19`）；应借鉴状态机/证据优先级而非规则实现。
- **不把 herdr 的 100ms polling/本地 socket 作为跨端唯一通道**：herdr server 轮询间隔 100ms、应用响应超时 5s、初始请求 5s（`src/api/server.rs:29-35`），且 socket 是本机权限模式 0600（`src/api/server.rs:87-93`）。这适合单机 daemon，不等价于 Se 的 Tauri、se-server、browser/iOS、remote 多表面；应抽象共享事件/快照契约，按 transport 适配。
- **不采用“事件可无条件重放覆盖 snapshot”**：herdr 文档自己要求 subscribe-first 后 authoritative reads；事件只用于失效，重连需 snapshot（`socket-api.mdx:118-126`）。Se 若跨远端/移动端，应显式 cursor/generation 和 snapshot reconcile，不凭 UI 事件顺序恢复。

## 采纳情况
not adopted；本条目仅记录研究证据与候选机制。未执行上游脚本、build、test 或安装器；测试均为静态源码证据。固定提交源码链接示例：`https://github.com/herdrdev/herdr/blob/5bd900d88c80d95f5b145397e45bbeea398b8165/src/pane/agent_detection.rs#L5-L13`。
