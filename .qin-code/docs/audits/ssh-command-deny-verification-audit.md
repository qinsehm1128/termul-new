---
schema: qin-doc/1
kind: audit
id: ssh-command-deny-verification
title: MCP-only SSH命令黑名单实施验收
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: current implementation, independent reviews, Goal verification and isolated browser fixture
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"18"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"19"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"20"}],"goal_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"goal-d0abd11f-f584-4a26-a910-d6f3b1231f9c"}]}
---

## 结论

SSH 命令黑名单完整实现并通过配置的机械验收。范围严格限定 MCP 的 `execute`、`job_start` 与 MCP managed-job 内部入口，不拦截普通本地终端、SSH 交互终端或 PTY。2026-11-07 17:02:32 +08:00，后续验收 Goal `goal-d0abd11f-f584-4a26-a910-d6f3b1231f9c` passed。功能提交后才进入正式版本发布。

## 发现

- 全局与 profile 拒绝规则取并集。支持原始文本 exact、contains 与有界 Rust regex，连接规则不能覆盖全局拒绝。
- v1 缺字段兼容迁移至 v2。每层最多32条、全策略最多128条、pattern最多512字节、command最多32KiB。非法规则拒绝发布，编译快照与授权使用同一不可变版本，CAS 保存先持久化再发布。
- 初始拒绝先于 worker 排队、凭据解析、联网或任务创建。执行和任务分发前再次检查当前授权及规则，规则变更不杀死已有工作，status/list/cancel 不被命令规则阻挡。
- 管理 UI 提供全局/profile 规则、只读继承、纯手动预览。保留已有授权、身份 pins、roots 和 POSIX consent。新授权默认13项操作，已有受限授权不扩大。
- Tauri `mcp_host_access_preview` 与 Web `POST /mcp/host/access/preview` 接入共享 Rust 预览，不保存草稿、不读凭据、不联网、不排队或创建任务。Web要求 authenticated LocalOperator，PublicTunnel禁止。
- 安全审查发现非法JSON/enum/字段可被框架错误回显的低风险问题，已在HTTP捕获JsonRejection并返回固定错误，在Tauri内部从Value解码后返回固定IPC错误。超限保留413，解码失败400，业务operation无效保留原HTTP200/failed-IPC约定。日志只记录稳定错误码。
- 首次新增测试误将业务operation错误期待为400，按既有业务契约修正为200且仍断言success=false、INVALID_ARGUMENTS及无sentinel回显。
- 首次完整Goal因macOS `/var` 临时根别名触发持久任务存储的符号链接祖先防护而失败。仅将测试fixture建立在canonical临时根，保留生产O_NOFOLLOW和所有安全断言。显式使用`TMPDIR=/var/folders/ww/wkjvt9cn5cxc8krpzn2mzqhw0000gn/T`的持久化回归通过。工具不能恢复failed Goal，后续Goal保留完全相同验收命令，最终通过。

## 证据

### 机械验收

以下完整链通过，未跳过或缩减检查：

```sh
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --test ssh_command_policy --test ssh_automation --test mcp_capabilities_acceptance --test mcp_host_web --test mcp_host_preview_web
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib ssh::
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib mcp_core::
cargo check --manifest-path src-tauri/Cargo.toml -p se-manager --no-default-features --features standalone-server --bin se-server
bun run typecheck
bunx vitest run src/renderer/components/settings/McpCommandDenyEditor.test.tsx src/renderer/components/settings/McpHostAccessPanel.test.tsx src/renderer/components/settings/McpHostAccessPanel.runtime.test.tsx src/renderer/lib/mcp-host-api.test.ts src/renderer/lib/__tests__/parity-checklist.test.ts
python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py
```

- HTTP生产router fixture：10项通过，覆盖纯预览、拒绝权限伪造、PublicTunnel、草稿不落盘、无凭据调用、错误不回显、body限额及缺服务显式错误。
- 前端5文件218项测试通过。12个相关TS/TSX/JSON文件Biome检查通过，git diff --check通过。
- `bun run build:frontend:tauri`及`bun run build:web`通过（任务255772svjx，前端源码此后未修改）。
- 插件目录329个registered commands，unclassified:0。
- 普通SSH connection、automation_backend、sftp、port_forward、core和se-pty路径的`git diff --exit-code`通过。源码边界回归与MCP行为拒绝测试互补。

### 独立审查

- `ssh-deny-ui-review`：当前v2 UI和双传输审查通过，无阻塞问题。规则/pins保留、13权限默认、MCP-only文字、纯预览与陈旧响应失效一致。
- `ssh-deny-security-review`：核心源代码未发现critical/high/medium绕过。收到精确diff后复核错误清洗修复，无剩余安全缺陷。指出的HTTP状态测试期望已按原契约纠正，并以10/10测试和最终完整Goal验证闭环。审查者未独立运行测试，执行证据来自主会话。

### 隔离浏览器布局

Vite alias将宿主facade替换为固定mock，不接触用户设置或真实SSH。Ego独立Space验证全局规则、profile继承及手动预览结果展示。桌面预览区clientWidth/scrollWidth均864。390px手机fixture展开连接后document宽度390/390、main宽度381/381，规则字段组均无横向溢出。预览结果为固定mock，只证明布局和交互，不作为匹配语义证据。

## 残留风险

- 原始命令文本黑名单不是shell沙箱，不能识别所有编码、解释器、脚本、别名或间接执行。文件操作仍由独立授权控制。
- 最后一次授权复查到后端真正执行之间不是远端shell原子撤销保证。
- 未连接真实SSH，未操作用户PTY。源码不改和fake行为测试不是手动终端端到端运行证明。
- Tauri实际桌面IPC运行时未做真实应用端到端测试，已通过命令注册、共享服务、facade测试与desktop编译路径验证。手机验证为390px浏览器fixture，不是物理手机测试。
- 构建仍有既有deprecated/unused/linker警告，未为通过验收更改无关代码。
- 本审计不代表新版本已发布或已安装。发布、远端产物及更新清单需另行验证。用户.gitignore和scripts/release/__pycache__不纳入本次提交。
