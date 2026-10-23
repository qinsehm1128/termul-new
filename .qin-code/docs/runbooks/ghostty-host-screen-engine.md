---
schema: qin-doc/1
kind: runbook
id: ghostty-host-screen-engine
title: 可选 Ghostty VT 宿主屏幕观察引擎构建与排错
date: 2026-10-23
status: draft
source: plan:ghostty-host-screen-engine; run:930423tv0q; run:590468e075; run:980691pjz3; run:980696fugi
updated: 2026-10-23
lang: zh-CN
---

## 适用

项目级 `terminalScreenEngine` 可选 `existing | ghostty-vt`，缺省 `existing`。仅真正新建的 PTY 解析当前项目设置，活体 adopt、冷恢复、项目切换和关闭客户端不热切换。PTY 管理、输入输出、xterm.js 和 iOS 渲染仍使用现有路径。

适用宿主为桌面 Terminal Core、桌面 shared-live、standalone `se-server`。web/iOS 只消费宿主能力、配置和状态，不加载本地 native 库，不扩大设置写权限。

## 步骤

### 固定构建工具链

建议 Python 3.12 或当前 Python 3.11，必须具备 tarfile data extraction filters。仅使用明确拥有的 cache 目录。

```bash
export GHOSTTY_TOOLCHAIN_CACHE=/path/to/owned/cache/se-ghostty
export SE_GHOSTTY_VT_ZIG="$(python3.12 scripts/release/prepare-ghostty-toolchain.py \
  --cache-dir "$GHOSTTY_TOOLCHAIN_CACHE" --print-executable)"
```

安装器固定 Zig 0.16.0 官方 URL、大小和 SHA256。支持 macOS arm64/Intel、Windows x64、Linux x64 构建宿主。Intel macOS 工具链仅用于交叉构建，不额外增加产品发布平台。损坏缓存或缺少安全解压能力明确失败，不关闭 TLS/哈希检查。

源码归档在 `src-tauri/vendor/ghostty-vt/`，Ghostty revision 为 `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`，MIT。归档 SHA256 为 `af8cd2fbd25c17990b25c8bf8352263c35c5ec446134220de867236497311edd`。MSVC 必须链接上游的 `ghostty-vt-static.lib`，不能误选 DLL 导入库 `ghostty-vt.lib`。

### 构建

```bash
# 开发默认无 Zig，明确报告 unsupported，不伪造空屏成功。
cargo test --manifest-path src-tauri/Cargo.toml -p se-ghostty-vt
# 必须真实 native。
cargo test --manifest-path src-tauri/Cargo.toml -p se-ghostty-vt --features native
# 生产配置含 ghostty-vt，canary 同样继承。
bun run build:tauri
# standalone release 先生成嵌入式 web。
bun run build:web
cargo build --manifest-path src-tauri/Cargo.toml --release \
  --bin se-server --features standalone-server,ghostty-vt
```

显式 `SE_GHOSTTY_VT_PREBUILT_DIR` 只接受 wrapper README 约定的严格 manifest 与库哈希。完整验收主动取消该变量，确保测试交付的源码构建，而非某台机器上的预构建捷径。

### 使用与身份

在项目设置页或侧栏项目设置对话框选择引擎。保存后新建终端验证。web/iOS 显示宿主偏好和实际状态，项目选择 `ghostty-vt` 本身不等于 ready。

生产采用可信当前可执行文件的 `--ghostty-vt-observer` 早期子进程模式，保留 server 单文件签名更新。不要把该二进制协议入口当交互式终端使用。只有 child 创建/使用 native VT，父进程绝不进程内 fallback。

Core build identity 覆盖额外 observer crate 的完整 Cargo 闭包及外置源码归档，二进制输入逐字节哈希。正式/nightly server 注入实际 checkout commit 身份。子握手必须核对身份，以防更新后的路径与长存活宿主不一致。

## 失败时

- `unsupported` / `legacy-host`：检查宿主版本和 native feature，不是检查浏览器本机。不要为隐藏该状态强制替换托管活 PTY 的旧 Core。
- `identity-mismatch`：可能是更新后的可执行文件与旧 Core 不一致。保持 PTY，仅降级观察器，不跳过握手。
- `overflow` / `timeout`：观察器跟不上输出，当前 PTY 永久 degraded，不自动重放不完整历史或重新启用。主输出必须继续。
- `crash` / `protocol` / `size-limit`：记录脱敏边界状态，不能记录屏幕正文、环境或凭据。不可用读取没有成功正文，不把陈旧快照当 ready。
- 下载/校验失败：修复网络或使用另一明确拥有的 cache，不绕过证书/哈希/native 测试。
- 回退项目为 `existing` 仅影响后续 spawn，不用设置保存或导航清理杀死活 PTY。

子进程是崩溃隔离，不是权限沙箱。禁止 PTY/剪贴板/文件回调及 Kitty 文件/临时文件/共享内存介质。无 PTY fd、凭据继承或写回协议。native 库必须无 main 前初始化，发布流水线在测试和生产构建后审计真实静态库。

## 验证

```bash
PYTHON=python3.12 bash scripts/verify-ghostty-host-screen.sh
```

完整验收要求 macOS/Xcode，以及 `QIN_CODE_SCRATCH_DIR` 或 `RUNNER_TEMP` 指向明确拥有的临时目录。脚本包含默认无 native、真实 VT、产物被动链接、PTY/Core 故障测试、真实 Core/server self-exec、真实 PTY resize/手机尺寸接管/关闭视图/claim 轮换、宿主 registry、WS 授权、双 facade、Agent 证据、类型检查、Swift 模型和 iOS Simulator 编译。

需要预先构建 native 宿主的两个 Rust 集成用例在默认 suite 中显式 ignored，但完整验收必须用 `native_self_exec -- --ignored --nocapture` 执行它们。缺 binary/build identity 直接失败，不静默跳过。

本机完整验收 `930423tv0q` 于 2026-10-23 14:18:00 +08:00 退出 0。Linux x64 真实 VT 交叉构建及 ELF64 产物审计通过。Windows 本机交叉构建因缺少 MSVC SDK 头文件失败，真实 Windows 产品运行待 CI，不记通过。native、配置、工具链和审计脚本有已完成的独立审查，最终 runtime 独立审查多次被提供方全局并发限制阻塞，不能冒充完成。

性能只记录边界微样本：macOS arm64 debug 下，128 个 4 KiB 块合计 512 KiB，关闭观察器的空边界约 1.2–2.1 µs，启用时入队约 2.8–5.0 ms，排空约 26–41 ms。80×24 空闲 child RSS 单次样本 6704 KiB。它们不是完整 PTY 吞吐或峰值内存基准，不宣称零开销。
