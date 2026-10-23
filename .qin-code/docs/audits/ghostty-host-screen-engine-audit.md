---
schema: qin-doc/1
kind: audit
id: ghostty-host-screen-engine
title: Ghostty 宿主屏幕观察引擎交付与验收审计
date: 2026-10-23
status: draft
source: goal:passed:2026-10-23T14:35:05+08:00; run:930423tv0q; run:590468e075; commits:a0444351,cba08b16
lang: zh-CN
---

## 结论

项目级可选 Ghostty VT 宿主屏幕观察引擎已实现并分步提交。`existing` 为缺省，PTY/xterm 输入输出保持既有路径，只有真正新建 PTY 解析项目偏好。桌面/shared-live/standalone/web/iOS 的适用配置、宿主状态、授权只读屏幕及 Agent 证据接线已完成。

2026-10-23 14:35:05 +08:00，Goal 正式执行 `bash scripts/verify-ghostty-host-screen.sh` 返回 passed。该机械结论不代表未执行平台测试或未完成独立审查已经通过。

## 发现

已修复的具体问题：

1. 原 native 实现选择 Windows 的 `ghostty-vt.lib`，上游该名称属于 DLL 导入库。改用 `ghostty-vt-static.lib` 并同步被动链接产物发现。
2. 实际 `PtyManager::resize_ioctl` 成功分支未向观察器提交 resize。补齐 ConPTY/Unix 两分支，并用真实 PTY 测试验证普通 resize、手机接管、停放桌面尺寸和恢复。
3. 冷恢复 facade 未记录宿主返回的原始观察模式。补齐 resume 的 host metadata/新 claim 记录，不查询或覆盖为当前项目偏好。
4. Terminal Core 入口直接链接 observer，但旧 build-id 只跟踪主 Core crate。增加额外 Cargo 根及其完整依赖闭包，纳入生产 feature 配置和外置固定源码。
5. 原构建身份把二进制归档有损解码为 UTF-8 再哈希。修为非可逆文本直接按原字节哈希，新增不同无效 UTF-8 字节不得产生相同身份的回归。
6. 协议尺寸预检与 native 单轴 1024 限制不一致。统一单轴及 262144 cells 上限，超限在创建/resize 前明确拒绝。
7. 工具链下载失败时 Windows 可能尝试删除未关闭的临时文件。改为所有失败路径先 close 后 unlink，增加 network/read/oversize 回归。旧 Python 缺安全解压能力时现在下载前报错。
8. Zig 准备步骤插入破坏既有版本戳与 lock 同步相邻约束。移到 lock 同步之后，原 parity 测试保持未弱化并通过。
9. Goal 执行器不继承普通工具的 scratch 环境变量。验收脚本新增自己拥有的 `~/.qin-code/scratch` 子目录 fallback，未跳过任何门禁。第一次 Goal 验收的该基础设施失败已保留，修复后的正式验收通过。

## 证据

### 本机机械验收

- TypeScript node/web/test 三工程类型检查通过。
- 默认 wrapper 不需要 Zig，明确 unavailable。真实 native 源码空 Cargo/Zig 缓存构建通过，1 unit、9 native integration、2 compile-fail doctests 通过。
- se-pty 默认 225、Terminal Core 32 项通过。协议测试覆盖超限帧、序列化及与 native 一致的尺寸限制。
- 实际 Core 和 server 可执行文件各执行 6 项 self-exec 测试，覆盖 ANSI/CR/光标、主/备用屏、resize、中文分块、协议/序号/barrier 拒绝、查询无写回和正文预算。
- 2 项必须显式执行的真实 native Rust 集成测试通过：监督器屏幕/barrier/子进程崩溃与回收，以及真实 PTY resize/手机模式/关闭视图后存活/claim 轮换。默认 suite 的 ignored 不构成绕过，完整脚本使用 `--ignored` 实际运行。
- host registry 旧默认和新偏好往返、真实 WS grant/generation 授权测试通过。生产 native GUI/Core/server 的 Rust 全入口检查通过。
- 配置、持久化、双设置入口、全部非 SSH spawn、Agent 宿主证据、Tauri/web facade、冷恢复、renderer root parity、host 状态 UI 测试通过。
- Swift 真实模型兼容脚本和 iOS Simulator 无签名 build 通过。
- 12 项工具链离线测试、14 项被动链接审计测试通过。审计器用真实 clang 生成的 Mach-O/ELF/COFF fixtures 验证拒绝构造器和 IFUNC/resolver。

完整普通工具执行 `930423tv0q` 于 14:18:00 +08:00 退出 0。提交后的 Goal 正式验收于 14:35:05 +08:00 passed。

### 平台产物

- macOS arm64：真实源码构建和运行通过，静态 archive 的 12 个 Mach-O64 对象被动链接审计通过。
- Linux x64：真实 Zig 交叉构建通过，12 个 ELF64 对象被动链接审计通过。这不是 Linux 上完整 server 的运行测试。
- Windows x64：本机交叉构建因没有 MSVC SDK 的 `cstring`/`stdlib.h` 等头文件失败。没有把该失败解释为 Windows runner 上必然失败，也没有宣称 Windows 产品构建/运行已验证。
- 正式/nightly 已接固定 Zig/native 测试/产物审计与 production feature，实际远端 CI 本轮未触发。

### 独立审查与开销

native wrapper、配置、工具链以及产物审计脚本已有有效独立审查。工具链两项中风险发现修复后已独立关闭。最终 runtime 和发布装配审查多次遭提供方 `gateway_concurrency_limit`，没有有效最终结论，不计为通过。父会话的补充检查发现并修复上述 resize 等问题，但不是独立审查的替代品。

macOS arm64 debug 边界微样本：512 KiB/128 块，关闭观察器空边界约 1.2–2.1 µs，启用入队约 2.8–5.0 ms，排空约 26–41 ms。80×24 空闲 native child RSS 单次样本为 6704 KiB。不是完整 PTY 吞吐或峰值内存基准，不宣称零开销。

### 提交

- `8d36414e`：项目配置、持久化与初始 native 文件。
- `06f0e5d0`：native 限额及被动链接说明。
- `f05d6214`、`df9fb6d5`：固定工具链与产物被动链接审计。
- `fb0f9a30`：隔离 runtime、授权 transport、真实 PTY 集成。
- `fd71fd05`、`2afbdacd`：跨端宿主证据与冷恢复接线。
- `17830339`：Windows 静态库与 grid 上限修正。
- `a0444351`、`cba08b16`：生产分发、完整机械验收及非交互 cache fallback。

早期 client commit 曾包含并发 native lane 已暂存的文件，全部属于本轮授权变更。未 reset/amend 共享历史，之后提交均限定路径。

## 残留风险

- Windows 真机/CI 构建运行及 Linux 完整 server 运行尚未执行。不能将本机交叉构建或静态 fixture 当成这些测试的替代。
- 最终 runtime 与发布装配独立审查仍受提供方并发限流阻塞。机械验收通过不代表该审查已完成。
- 独立子进程提供崩溃隔离，不是 OS 权限沙箱。禁用 host callbacks/Kitty 介质、清环境和 fd 隔离降低副作用面，但不等于移除进程的全部环境权限。
- 本轮未发布安装包、未部署、未打开 PR。原有未提交 `.qin-code/knowledge/` 与 reference lock 保持不变。
