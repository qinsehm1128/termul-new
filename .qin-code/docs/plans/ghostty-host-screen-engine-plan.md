---
schema: qin-doc/1
kind: plan
id: ghostty-host-screen-engine
title: 项目级可选 Ghostty VT 宿主屏幕观察引擎实施计划
date: 2026-10-23
status: draft
source: user:approved-implementation; baseline:5b1782d4; agent:ghostty-implementation-plan; agent:ghostty-server-packaging-opinion
updated: 2026-10-23
lang: zh-CN
---

## 目标

实现已批准的方案 A：项目级可选的 Ghostty VT 宿主屏幕观察引擎。配置字段 `terminalScreenEngine` 为 `existing | ghostty-vt`，缺省 `existing`。PTY 管理、输入输出与 xterm.js 不变。新 PTY 在发布前固定模式，修改设置、切换项目、adopt 活 PTY 不热切换、不杀会话。真正重新 spawn 才重新解析项目设置。

产品基线 `8662be45`（0.15.21），研究基线 `5b1782d4`。已有未提交 `.qin-code/knowledge/` 与 reference lock 不属于产品变更。

### 不可破坏的边界

- 原生 VT 仅在独立子进程中初始化和调用，不允许进程内降级执行。观察器崩溃不能带走 PTY。
- 不传递 PTY fd，不注册 WRITE_PTY、剪贴板、文件或其它宿主副作用回调。保留原 reader 的 DAFilter 行为，不额外回写。
- 观察器属于 PtyManager/Core 生命周期而非 GUI。GUI 退出、项目切换、客户端断开不销毁 Core 所有的活 PTY。
- 队列和资源有界。观察器落后、失败、超限时明确降级，不阻塞或丢失主输出，不以陈旧快照冒充 ready。
- 项目配置走宿主持久化，shared-live 摘要与 standalone 注册表一致。`projectId` 只归因，不是授权。
- 读屏复用已签发终端的 claim/generation/scope 校验，不能新增 claim-free 正文入口。
- 桌面、shared-live、standalone、浏览器/手机与原生 iOS 一起落实适用契约。不能用“以后补 parity”代替实现。

## 非目标

- 不替换 renderer、PTY 或输入协议，不引入 Ghostty GPU/窗口/WASM 渲染。
- 不热启用旧 PTY，不通过不完整 replay 伪造初始屏幕，不建设新的 Agent 编排/wait API。
- 不默认持久化终端正文、环境或凭据，不扩展 web/iOS 的项目设置写权限。
- 子进程是崩溃隔离，不宣称权限沙箱。必须禁用宿主回调和 Kitty 文件/临时文件/共享内存介质。
- 不以默认 feature 关闭为理由交付只能运行 unavailable stub 的生产开关。

## 任务

### 1. 已完成调查与决策

配置、运行时、固定上游源码分别有独立调查。准确实现规划见 `agent://ghostty-implementation-plan`，分发权衡独立意见见 `agent://ghostty-server-packaging-opinion`。规划核实 manager 中没有可直接复用的 emulator/ScreenObservation，不能依赖早期调查的该假设。

固定源：herdr `5bd900d88c80d95f5b145397e45bbeea398b8165` 中的 `vendor/libghostty-vt`，Ghostty `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`，版本 `1.3.2-HEAD-+44f2a44df`，MIT。确定性源码归档 SHA256 `af8cd2fbd25c17990b25c8bf8352263c35c5ec446134220de867236497311edd`，3,939,039 字节。原始 dist 是上游作者本地产物，不杜撰公开 tarball URL。源码/许可/pin 必须随仓库交付。

Zig 固定 `0.16.0`，不全局安装。`scripts/release/prepare-ghostty-toolchain.py` 固定官方 macOS arm64、Windows x64、Linux x64 URL、大小和 SHA256，仅安装到显式指定 cache。Cargo 默认构建不需要 Zig，生产 `ghostty-vt` feature 显式转发真实 native。原生最小 wrapper 是独立 `se-ghostty-vt` crate，`native` feature 才链接真实 FFI。

### 2. 观察器协议与状态

新中立协议/runner crate `se-ghostty-observer` 由 runtime lane 实现。每个开启的 PTY 有一个子进程。v1 为 `u32` 大端长度加 JSON，分配前校验，单帧最大 512 KiB。stdout 仅协议，stderr 仅脱敏诊断。

- `Hello {protocolVersion, observerBuildId, ghosttyRevision}`。
- stdin 依次接收 `Init {cols, rows}`、`Output {eventSeq, dataBase64}`、`Resize {eventSeq, cols, rows}`、`Read {requestId, barrierSeq, maxLines}`。
- stdout 为 `Ack {appliedSeq}` 与 `Reply {requestId, appliedSeq, text, truncated}`。
- Init 必须在发布 PTY/启动 reader 前进入本地队列，不等待 FFI。
- post-DAFilter 字节和成功 resize 在统一短临界区提交序号。锁内只做内存操作，不 read/ioctl/pipe IO/await。语义是宿主提交点线性化，不虚称知道内核字节生成时刻。
- 队列含在途最多 4 MiB/256 帧，全局最多 30 个 helper。网格最多 262144 cells，scrollback 为零。
- Read 最多 200 行、64 KiB 文本，仅一个在途，额外请求返回 busy。
- 握手或有积压无进展 2 秒即降级。overflow、崩溃、失序、超时、协议超限永久 degraded，清空快照并有界 kill/reap，不自动重启。

公开状态容器为 `screenObservation`，字段为 `terminalScreenEngine`、`state`、`reason`、`capturedSeq`、`appliedSeq`、`observerBuildId`。

- state：`disabled | starting | catching-up | ready | degraded | unsupported`。
- reason：null 或 `helper-missing | identity-mismatch | overflow | timeout | crash | protocol | size-limit | legacy-host`。
- ready 必须完成初始化且 capturedSeq 等于 appliedSeq。迟到 Reply 不能逆转 degraded。

### 3. 分发决定：可信 self-exec 的子进程模式

采用独立审查建议 B：现有签名可执行文件以严格的 `--ghostty-vt-observer` 早期模式自执行。桌面 `se-terminal-core`、standalone `se-server` 与 GUI 的 Core fallback 均在常规配置、监听器、Tauri 初始化前分派。

这仍是真正的独立进程，不是进程内 FFI 调用。父可被动静态链接 native 符号，但必须核实无 main 前构造器/初始化，且所有 Terminal 创建/解析/销毁只在 child 分支发生。若无法证明被动链接，则回退为独立 helper image 并重新封口升级装配，不能降低隔离要求。

- 仅可信 current_exe/宿主位置，不搜索不可信 cwd 或 PATH。
- child `env_clear`，只补必要 OS 变量，不继承凭据或 PTY fd。
- 握手校验协议与 build identity，防止单文件更新后路径指向新版本。
- 不新增第五个独立更新组件，保持 `se-server` 单文件签名自更新契约。
- 外置 `vendor/ghostty-vt` 不在 Cargo crate 闭包内，须显式纳入现有宿主组件 build-id 输入。

### 4. 并行实施与文件所有权

| Lane | 所有权 | 交付 |
| --- | --- | --- |
| native | `src-tauri/crates/se-ghostty-vt/**`、`src-tauri/vendor/ghostty-vt/**` | 固定源码/许可、C shim、默认 unavailable 和真实 native、真实 VT fixtures、被动链接审计 |
| runtime | 其余 `src-tauri/**` Rust/Cargo manifest，不改 lock | 协议 runner、supervisor、有序 tap、registry/host spawn、状态与授权读屏、各入口早期 child dispatch、故障测试 |
| client | `src/**` TS/React/locales、`ios/SeRemote/**`、Swift 验证脚本 | Project/PersistedProject、结构比较、两个设置入口、7 个非 SSH spawn、摘要、Agent 证据、状态/unsupported、Swift 缺省解码 |
| parent | release/CI 脚本、build-id、最终 lock、组合验收、文档 | 支持构建真实 native、跨端组合检查、独立 review、分步提交 |

web 项目终端从宿主 registry 解析偏好，不扩大 intent 权限。iOS 继续遵守现有只读设置权限，但解码和呈现宿主配置/适用状态。两个 renderer root 维持一致。

### 5. 验证与提交

每个模块先跑最小相关测试。native 必须在 fresh 私有构建/cache 条件下验证真实 parser，不能依赖未交付的 scratch 文件或把 stub 测试当作 native 证据。收尾跑关闭/开启成本对照，记录真实数值，不宣称零开销。

已验证改动前基线：PTY/Core lib、前端四文件 80 项、TypeScript 三工程、发布 49 项、Xcode 27 的 iOS Simulator 无签名 build。新工具链离线测试 8 项通过，真实 macOS 官方归档的校验/解压/版本/缓存复用通过。外置源码身份输入修改后 32 项 build-id 守卫通过。这些不等于最终产品验收。

## 验收

机械入口已配置为 `bash scripts/verify-ghostty-host-screen.sh`。脚本当前保留显式失败 guard，在 observer/transport/Swift 必需 gates 未接齐前绝不假通过。交付前去除 guard 的前提是实际补齐对应测试，而不是删掉失败条件。

1. 旧/缺省配置为 existing，不启动观察器，不改变原 PTY 字节/输入/Agent 默认判断。
2. 项目配置保存重启保留，shared-live/standalone/web/iOS 缺省兼容，两个设置入口一致。
3. 仅新 PTY 解析模式，活体 adopt 和项目切换不热改、不 kill/recreate。
4. 真实 VT 覆盖 ANSI、CR/光标覆盖、主/备用屏、resize、中文/分块，并能读取未打开 renderer 的屏幕证据。
5. 无重复 PTY 回写/剪贴板/响铃/文件副作用。
6. helper 缺失、crash、hang、失序、超限、队列满均有明确状态与日志，PTY 继续可读写。
7. 屏幕证据必须 ready/序号一致，故障不回退为伪造成功，不增加 claim-free 正文读取。
8. 真实 native、PTY/Core 故障隔离、两 transport、host registry/持久化、renderer typecheck、Swift 模型和 iOS build 均有新鲜证据。
9. 正式/nightly 的 macOS arm64、Windows x64、Linux standalone 支持构建启用真实 native。无法在本机执行的平台必须准确标记由 CI 验证，不能宣称本机通过。
10. 独立审查无未处理高风险发现，分步提交只包含本轮文件。
