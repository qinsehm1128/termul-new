---
schema: qin-doc/1
kind: audit
id: ghostty-host-screen-engine
title: Ghostty 宿主屏幕观察引擎交付与验收审计
date: 2026-10-23
status: draft
source: goal:passed:2026-10-23T14:35:05+08:00; agent://ghostty-final-review-retry; run:930423tv0q
updated: 2026-10-23
lang: zh-CN
---

## 结论

项目级可选 Ghostty VT 宿主屏幕观察引擎已实现并分步提交。`existing` 为缺省，PTY/xterm 输入输出保持既有路径，只有真正新建 PTY 解析项目偏好。桌面/shared-live/standalone/web/iOS 的适用配置、宿主状态、授权只读屏幕及 Agent 证据接线已完成。

- **机械验收通过**：2026-10-23 14:35:05 +08:00，Goal 正式执行 `bash scripts/verify-ghostty-host-screen.sh` 返回 passed。
- **最终独立审查通过**：2026-10-23 14:59:05 +08:00，`ghostty-final-review-retry` 返回 approved，无具体正确性、安全、生命周期、授权或分发阻断发现。
- 独立审查为限定范围源码/diff 检查，没有重新运行测试。机械验收与源码审查不能代替尚未执行的 Windows 产品运行和 Linux 完整 server 运行验证。

## 发现

### 已修复的具体问题

1. Windows 原 native 实现选择 `ghostty-vt.lib`，但该名称属于上游 DLL 导入库。改用 `ghostty-vt-static.lib` 并同步被动链接产物发现。
2. 实际 `PtyManager::resize_ioctl` 成功分支未向观察器提交 resize。补齐 ConPTY/Unix 两分支，用真实 PTY 验证普通 resize、手机接管、停放桌面尺寸与恢复。
3. 冷恢复 facade 未记录宿主原始观察模式。补齐 resume 的 host metadata/新 claim 记录，不用当前项目偏好覆盖活体模式。
4. Terminal Core 入口直接链接 observer，但旧 build-id 只跟踪主 Core crate。增加额外 Cargo 根及完整依赖闭包，纳入生产 feature 配置与外置固定源码。
5. 构建身份将二进制归档有损解码为 UTF-8 后哈希。修为非可逆文本按原字节哈希，增加不同无效 UTF-8 字节不得产生相同身份的回归。
6. 协议尺寸预检与 native 单轴 1024 限制不一致。统一单轴及 262144 cells 限制，在创建/resize 前明确拒绝超限。
7. Windows 下载失败可能尝试删除未关闭的临时文件。所有失败路径先 close 后 unlink，增加 network/read/oversize 回归。旧 Python 缺安全解压能力时下载前报错。
8. Zig 准备步骤破坏版本戳与 lock 同步相邻约束。移到 lock 同步之后，既有 parity 测试保持未弱化并通过。
9. Goal 执行器不继承普通工具的 scratch 变量。验收入口新增自己拥有的 `~/.qin-code/scratch` 子目录 fallback，未跳过门禁。初次基础设施失败保留，修复后正式验收通过。

### 最终独立审查结论

审查者确认以下实际接线，无需追加代码修复：

- output/resize 在统一状态与序号门内提交，`try_send`、帧数/字节预算和故障降级不会阻塞 PTY producer。
- Windows 与 portable-PTY 两条路径均在真实 resize 成功后发送相同网格给观察器，PTY 保持权威。
- 协议、身份、序号和 read barrier 不匹配会被拒绝。单个在途读取仅在 captured/applied 序号一致时返回可信正文。
- claim/generation/conversation scope 在 native await 前后复验，轮换或终止后的旧正文不能返回。
- native 访问限制在 child 协议进程，三条宿主入口都在普通应用初始化前精确分派 self-exec 模式，没有 PTY 写回。
- 生产 feature、observer 额外依赖闭包、Cargo.lock 和固定源码纳入分发与身份路径。

此前多次 `gateway_concurrency_limit` 失败不计作审查通过。14:59 的有效独立结论已补齐该阻塞项。

## 证据

### 本机机械验收

- TypeScript node/web/test 三工程类型检查通过。
- 默认 wrapper 无需 Zig，明确 unavailable。真实 native 空 Cargo/Zig 缓存源码构建通过，1 unit、9 native integration、2 compile-fail doctests 通过。
- se-pty 默认 225 项、Terminal Core 32 项通过。协议覆盖超限帧、序列化及与 native 一致的 grid 限制。
- 实际 Core/server 可执行文件各 6 项 self-exec 测试通过，覆盖 ANSI/CR/光标、主/备用屏、resize、中文分块、协议/序号/barrier 拒绝、查询无写回及正文预算。
- 2 项真实 native Rust 集成通过：监督器屏幕/barrier/子进程崩溃回收，以及真实 PTY resize/手机模式/关闭视图后存活/claim 轮换。它们在默认 suite 中显式 ignored，但完整验收实际使用 `--ignored` 执行，不是跳过验收。
- host registry 旧默认/新偏好往返、真实 WS grant/generation 授权通过。生产 native GUI/Core/server 全入口 Rust 检查通过。
- 项目配置、持久化、双设置入口、非 SSH spawn、Agent 宿主证据、Tauri/web facade、冷恢复、renderer parity 和状态 UI 测试通过。
- Swift 真实模型兼容与 iOS Simulator 无签名 build 通过。
- 工具链 12 项离线测试、被动链接审计 14 项测试通过。真实 clang Mach-O/ELF/COFF fixtures 验证构造器与 IFUNC/resolver 拒绝。

普通工具执行 `930423tv0q` 于 14:18:00 +08:00 退出 0。提交后的正式 Goal 验收于 14:35:05 +08:00 passed。

### 平台与独立证据

- macOS arm64：真实源码构建运行通过，12 个 Mach-O64 archive 对象被动链接审计通过。
- Linux x64：真实 Zig 交叉构建通过，12 个 ELF64 对象审计通过。不是 Linux 完整 server 的运行测试。
- Windows x64：本机交叉构建缺 MSVC SDK 的 `cstring`/`stdlib.h` 等头文件而失败。未宣称 Windows 产品构建或运行已验证。
- 正式/nightly 已接固定 Zig/native 测试/产物审计和 production feature，本轮未触发远端 CI。
- native wrapper、配置、工具链、产物审计脚本已有有效独立审查。工具链两项中风险发现修复后独立关闭。
- 最终审查 `agent://ghostty-final-review-retry`：approved、findings=[]，覆盖所要求的 runtime/protocol/self-exec/authorization/workflow/production config/component identity 源码与 diff。未重跑测试，使用已有验收输出作为外部证据。

### 开销样本

macOS arm64 debug：128 个 4 KiB 块共 512 KiB，关闭观察器空边界约 1.2–2.1 µs，启用入队约 2.8–5.0 ms，排空约 26–41 ms。80×24 空闲 child RSS 单次样本 6704 KiB。不是完整 PTY 吞吐或峰值内存基准，不声称零开销。

### 提交

- `8d36414e`、`06f0e5d0`：项目配置、初始 native、限额与被动链接说明。
- `f05d6214`、`df9fb6d5`：固定工具链与产物审计。
- `fb0f9a30`：隔离 runtime、授权 transport、真实 PTY 集成。
- `fd71fd05`、`2afbdacd`：跨端宿主证据与冷恢复。
- `17830339`：Windows 静态库/grid 修正。
- `a0444351`、`cba08b16`：生产分发、完整验收、非交互 cache fallback。
- `59d92324`：首次最终验收审计。此文随后按有效独立审查结果更新。

早期 client commit 包含并发 native lane 已暂存的文件，均属于本轮授权变更。未 reset/amend 共享历史，后续提交均限定路径。

## 残留风险

- Windows 真机/CI 产品构建运行及 Linux 完整 server 运行未执行。静态 fixture、本机交叉构建或源码审查都不能替代。
- 子进程提供崩溃隔离，不是 OS 权限沙箱。禁用宿主回调/Kitty 介质、清环境与 fd 隔离不等于移除全部环境权限。
- 本轮未发布安装包、未部署、未打开 PR。既有未提交 `.qin-code/knowledge/` 与 reference lock 保持不变。
