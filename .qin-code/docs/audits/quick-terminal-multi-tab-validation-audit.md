---
schema: qin-doc/1
kind: audit
id: quick-terminal-multi-tab-validation
title: 快捷终端多终端与统一标签验收记录
date: 2026-10-26
status: draft
source: user:2026-10-26T15:14:24+08:00; agent://quick-tabs-host-integration; agent://quick-tabs-editor-final-review; agent://quick-tabs-draft-isolation; agent://quick-tabs-drafts-final-review; bg:7304300t5b; bg:762299i399; bg:874137nrme; commit:c2ade681; commit:ed36b611
updated: 2026-10-26
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-close-safety"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-drafts"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-editor-safety"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-host"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-ui"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-verify"}],"goal_refs":[]}
---

## 结论

同一快捷条目内多个终端、终端与文件统一标签工作区、快捷草稿与普通项目状态隔离已实现并通过验收。提交为 `c2ade681`（共享契约、宿主及双端 API）和 `ed36b611`（统一标签 UI、编辑与持久化安全）。独立审查的 RV001–RV006 均已关闭，无剩余阻塞发现。

最终相关回归为 **20 个测试文件、372 项通过、17 项原有跳过**。node/web/test 类型检查、25 个改动 renderer 文件的 Biome 检查、Web 与 desktop 前端生产构建通过。真实 CodeMirror 浏览器夹具及 390px 手机布局验证通过。**未发布、未安装、未重启 Se，未操作用户真实 PTY。** 本轮隔离浏览器空间和 fixture 服务已关闭。

## 发现

### 功能与跨端

- 宿主 schema v2 持有有序 `terminalIds`，保留首项兼容别名。v1 内存兼容读取，修改后持久化为 v2。不可读/未来格式保护原文件。
- 新建、单关、全关、删除经共享 Rust 服务和 Core IPC，同时接通 Tauri 与带鉴权的 flat HTTP 路由。单关校验条目归属，不终止兄弟终端。
- 每条目使用独立终端/文件标签布局，未选中终端保持挂载。导航、文件预览和恢复标签不替换活 PTY。窄屏使用横向滚动标签和返回列表入口。
- dead/stale terminal 需用户显式重开才创建替代终端，失败后的再次重试保持替代意图，不抢占兄弟 PTY。

### 安全修复与审查关闭

- RV001：文件编辑器按路径隔离实例，切换前 flush，修复 debounce 缓冲从 A 写入 B 的风险。
- RV002：关闭前 flush 后重读 dirty/busy 状态，导航清理也先 flush，禁止静默丢弃未保存编辑。
- RV003：原生菜单与 DOM 关闭按当前路由派发，空快捷工作区不落入隐藏项目关闭路径。每次 close claim 有独立标识，旧 cleanup 不能移除新 owner。另修复保存快捷键误保存隐藏项目文件。
- RV004：恢复文件使用非激活读取，完成时校验当前工作区与标签归属。editor-store 的读取完成保护复用期间出现的新缓冲，不以迟到磁盘数据覆盖它。
- RV005：文件读取失败有可重试/关闭 UI，日志不含文件内容、标识、路径或原始错误正文。
- RV006：基于显式快捷所有权与项目 pane 引用筛选项目持久化，保留共享引用，隔离 quick-only 缓冲及 active selection。快捷 dirty draft 独立存于 `quick-terminal/editor-drafts/{entryId}` version 1，恢复不激活文件且不覆盖更新的内存。
- 草稿后续复核修复两处竞态：项目 payload 读取后和每个文件 open await 后均重新检查 live quick dirty/busy；尚未完成 draft 读取时也记录 dirty→clean 撤销，防止旧草稿复活。两项均有回归并获独立复核批准。

## 证据

### 自动化与独立审查

- Rust：`se-quick-terminal` 23 项，Core quick-terminal 3 项，manager quick-terminal 11 项，terminal-tab 4 项，auth 20 项均通过。筛选存在重叠，不合并为独立总数。后端提交后未再变更。
- 独立宿主集成另执行 21 项 TypeScript 与 27 项 Rust 契约/路由/鉴权检查，通过且无阻塞发现。
- 最终父回归 `bg:7304300t5b`：20 文件、372 项通过、17 项既有跳过。范围包含全部 quick-terminal tests、项目编辑持久化、active-tab 隔离、editor-store、WorkspaceLayout、关闭持久化、file-open-target、跨端 checklist 和 renderer-root parity。
- 同一命令内 `bun run typecheck` 的 node/web/test 三项、25 文件 `biome check` 以及 `git diff --check` 全部通过。提交钩子再次检查 25 文件通过。
- `agent://quick-tabs-editor-final-review`：RV001–005 approved，0 actionable findings。独立执行 93 项测试及 web/test 类型检查通过。其报告中的 import-order 错误已由父修正，最终全范围 lint 通过。
- `agent://quick-tabs-drafts-final-review`：RV006 closed/approved，0 findings。修复后独立执行草稿及项目持久化 49 项测试通过，并核对两处竞态和显式所有权逻辑。
- `cargo check -p se-manager` 和 `cargo check -p se-manager --bin se-server --features standalone-server` 通过。
- 最终 Web 与 desktop Vite production build `bg:762299i399` 通过，产物 `/Users/qs/.qin-code/scratch/quick-tabs-delivered-build-g1biOS5K/{web,desktop}`，不覆盖运行中的应用。

### 真实浏览器交互

夹具目录：`/Users/qs/.qin-code/scratch/quick-tabs-browser-atZvgqbE`。挂载真实 QuickTerminalsPage、workspace store、标签组件与 CodeMirror，宿主、磁盘、持久化及终端渲染面为隔离替身，不等同于安装应用和真实 PTY 端到端验收。

- 同条目创建第二终端时，第一终端 DOM 身份及输入保持。文件打开进入统一标签，可返回原终端。单关第二终端不影响第一终端。
- 草稿跨标签保留，未保存关闭被阻止，保存后可关闭。切条目、离开重入和刷新保留宿主 ID，记录显示 reuse 而非重复 spawn。
- Cmd+T/Cmd+W 增关当前条目内终端，不改变左侧条目数量。
- 最终真实 CodeMirror 同内容同语言 A/B 验证：输入后 **1.6ms** 内切换，A 内容只写入 A，B 不变。输入后 **1.8ms** 内关闭，dirty 缓冲保留。立即离开后再次进入并完整刷新，dirty draft 仍恢复。证据 `rapid-editor-results.json`，最终运行时间 2026-10-26 16:52 +08:00。
- 人为注入恢复文件不存在时出现错误 UI 且 activePath=null，无未处理异常；解除故障后重试恢复。延迟读取在离开后完成，不抢占新 owner。证据 `recovery-results.json`，最终运行同为 16:52 +08:00。
- 390px 窄屏：body 宽度 390，tab viewport 354、内容 1504，overflow-x=auto，实际横向滚动到 1149.5，草稿可见且终端 ID 不变。证据 `mobile-final-results.json`。
- 截图 `desktop-final-draft.png`、`mobile-final-draft.png` 已读取核对。基础流程截图 `desktop-terminal-tabs.png`、`mobile-file-tabs.png` 保留。
- 已完成验收步骤无应用运行时错误。一次 Ego 控制调用遇到执行上下文失效，读取现有页面并重新导航绑定上下文后完成手机验收，非应用错误。

### 非阻塞输出与清理

- 现有 React Router future-flag、checklist 的 System/Keyboard 推荐覆盖、布局测试 act 警告，以及 SSH 密码输入 autoFocus lint 警告未隐藏。本轮未新增 skip。
- Ego Lite 提示可更新，未升级。仅关闭本轮 `quick-tabs-isolated-20261026` TaskSpace 和 `quick-tabs-fixture` 服务，未动用户标签页、Se 应用或真实会话。
- 与当前任务无关的 mouse-install audit、本地知识/锁文件、release pycache 未提交或删除。

## 残留风险

- 未在安装中的应用内执行新功能，未进行原生 Windows 运行验证。PTY 生命周期由 Rust 隔离测试覆盖，浏览器使用替身。
- 删除条目后的草稿仍保留在条目 key 下。本轮不增加孤立草稿恢复界面或垃圾回收策略，避免删除用户编辑。
- 继续使用既有 debounced persistence。进程在写入完成前突然终止的耐久性边界未消除。
- 本轮不新增普通修改的跨客户端 durable-list broadcast。
- 未运行全仓库所有测试。当前证据覆盖相关功能、编辑器与跨端集成范围。
