---
schema: qin-doc/1
kind: runbook
id: ghostty-host-screen-engine
title: 可选 Ghostty VT 宿主屏幕观察引擎构建与排错
date: 2026-10-23
status: draft
source: goal:passed:2026-10-23T14:35:05+08:00; agent://ghostty-final-review-retry; audit:ghostty-host-screen-engine
updated: 2026-10-23
lang: zh-CN
---

## 适用

项目级 `terminalScreenEngine` 为 `existing | ghostty-vt`，缺省 `existing`。仅真正新建 PTY 解析项目设置，adopt、冷恢复、切换项目、关闭视图不热切换或重建会话。PTY 输入输出、xterm.js 与 iOS 渲染保持原路径。

适用宿主为桌面 Terminal Core、桌面 shared-live 和 standalone `se-server`。web/iOS 消费宿主配置与实际状态，不加载本地 native 库，不扩展设置写权限。

## 步骤

### 固定工具链与构建

建议 Python 3.12 或当前 Python 3.11，必须有 tarfile data extraction filters。安装器固定 Zig 0.16.0 官方 URL、大小和 SHA256，仅写入明确拥有的 cache，不全局安装或绕过 TLS/哈希。

```bash
export GHOSTTY_TOOLCHAIN_CACHE=/path/to/owned/cache/se-ghostty
export SE_GHOSTTY_VT_ZIG="$(python3.12 scripts/release/prepare-ghostty-toolchain.py \
  --cache-dir "$GHOSTTY_TOOLCHAIN_CACHE" --print-executable)"

# 默认开发构建不需要 Zig，明确 unavailable。
cargo test --manifest-path src-tauri/Cargo.toml -p se-ghostty-vt
# 真实 native 行为。
cargo test --manifest-path src-tauri/Cargo.toml -p se-ghostty-vt --features native
# 桌面/canary 的生产配置已选 ghostty-vt。
bun run build:tauri
# standalone release 先准备嵌入式 web。
bun run build:web
cargo build --manifest-path src-tauri/Cargo.toml --release \
  --bin se-server --features standalone-server,ghostty-vt
```

固定 MIT 源码在 `src-tauri/vendor/ghostty-vt/`。Ghostty revision：`44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`。归档 SHA256：`af8cd2fbd25c17990b25c8bf8352263c35c5ec446134220de867236497311edd`。Windows 必须链接 `ghostty-vt-static.lib`，不是 DLL 导入库 `ghostty-vt.lib`。

`SE_GHOSTTY_VT_PREBUILT_DIR` 仅接受 wrapper README 规定的严格 manifest/库哈希。完整验收主动取消该变量，测试交付的源码构建。

### 使用

在项目设置页或侧栏项目设置选择宿主屏幕引擎，保存后新建终端。项目选择 `ghostty-vt` 不等于终端 ready，UI/Agent 必须消费真实宿主状态。冷恢复保留宿主原模式和新签发的 claim，不用当前项目偏好覆盖。

隔离采用现有可信可执行文件的 `--ghostty-vt-observer` 早期子模式。不要把它作为交互式终端运行。只有 child 创建/使用 VT，父进程不调用 FFI、不进程内 fallback。Core 身份覆盖 observer 完整 Cargo 闭包、生产 feature 配置及外置源码归档，server 发布注入实际 commit 身份。握手不得跳过身份校验。

## 失败时

- `unsupported` / `legacy-host`：检查宿主版本/构建能力，不是浏览器本机。不能强制替换仍托管活 PTY 的旧 Core。
- `identity-mismatch`：可能是更新后的可执行文件与旧 Core 不匹配。保持 PTY，仅降级观察器。
- `overflow` / `timeout`：当前观察器永久 degraded，不自动重放不完整历史。主输出继续，不使用陈旧快照。
- `crash` / `protocol` / `size-limit`：记录脱敏状态，不记录正文、环境或凭据。不可用读取不伪造成功正文。
- 下载或缓存校验失败：修复网络或换一个明确拥有的 cache，不关校验、不跳过 native 测试。
- 回退为 `existing` 只影响后续 spawn，不能通过保存设置/导航清理杀死活 PTY。

子进程是崩溃隔离，不是权限沙箱。没有 PTY fd 或写回协议，不注册宿主副作用回调，禁止 Kitty 文件/临时文件/共享内存介质。生产流水线审计 native 静态库无 main 前初始化。

## 验证

```bash
PYTHON=python3.12 bash scripts/verify-ghostty-host-screen.sh
```

完整验收要求 macOS 与 Xcode。可设置 `QIN_CODE_SCRATCH_DIR`/`RUNNER_TEMP`，没有时自动使用自己拥有的 `~/.qin-code/scratch` 缓存子目录和随机 iOS build 目录，适用于不继承交互环境的 Goal 执行器。

门禁涵盖：默认无 native、真实 VT、产物被动链接、PTY/Core 故障、真实 Core/server self-exec、真实 PTY resize/手机接管/关闭视图/claim 轮换、registry、WS 授权、Tauri/web facade、Agent 证据、冷恢复、类型检查、Swift 模型及 iOS 编译。两个需要真实宿主 binary 的 Rust 用例在默认 suite 中 ignored，但本脚本必须用 `native_self_exec -- --ignored --nocapture` 执行，不提供 binary/identity 会失败。

- 正式 Goal 机械验收于 **2026-10-23 14:35:05 +08:00 passed**。
- 最终 runtime/发布装配独立审查于 **2026-10-23 14:59:05 +08:00 approved**，无阻断发现。该审查为限定源码/diff 检查，没有独立重跑测试。
- Linux 真实静态库交叉构建/审计通过，不等于完整 server 的 Linux 运行测试。Windows 本机交叉构建因缺少 MSVC SDK 头文件失败，Windows 产品运行和 Linux 完整 server 运行仍待对应环境验证。

完整证据见 `.qin-code/docs/audits/ghostty-host-screen-engine-audit.md`。

性能为非发布级微样本：macOS arm64 debug，512 KiB 入队约 2.8–5.0 ms，排空约 26–41 ms；关闭观察器空边界约 1.2–2.1 µs。80×24 空闲 child RSS 单次样本 6704 KiB。不是完整 PTY 吞吐或峰值内存基准，不声称零开销。
