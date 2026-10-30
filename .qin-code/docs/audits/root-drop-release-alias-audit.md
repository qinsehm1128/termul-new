---
schema: qin-doc/1
kind: audit
id: root-drop-release-alias
title: v0.15.27发布与独立Canary别名安装验收
date: 2026-10-30
status: draft
source: user:2026-10-30T08:52:57+08:00; commit:8b4f1ed1; github-run:40984099110; agent://root-drop-canary-artifact-review; file:/Users/qs/.qin-code/scratch/root-drop-release-alias-0n4eep6o/final-install-acceptance.json
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-alias-install"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-alias-prepare"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-preflight"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-publish"}],"goal_refs":[]}
---

## 结论

**发布与独立别名安装均已完成，正在运行的正式 Se 未被替换或重启。**

- 正式 Release：[v0.15.27](https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.27)，源码 `8b4f1ed1ee1a95de4288e81312cb525d744abe7e`，包含根目录拖放修复 `0d48d768`。
- 别名安装：`/Users/qs/Applications/Se Manager Canary.app`，`0.15.27-canary.1`，独立 identifier `com.se-manager.app.canary`。
- 11:04:40 +08:00 完成旧 0.15.25-canary.1 备份与原子替换；11:04:59 独立安装 verifier 通过；11:12:56 父会话再次验证通过。
- `/Applications/Se Manager.app` 仍为 0.15.24。主 PID `8769`、核心 PID `8849`、`8855`、`8857` 的 PPID、可执行路径与 2026-10-26 启动时间全部保持。
- 正式版及系统目录旧 Canary 的完整文件树未变，Canary 持久数据未变。
- 未启动任何 Se/Canary，没有停止、重启、发送应用进程信号或向活 PTY 输入。新别名保持未启动。

## 发现

### 发布与独立构建

版本三源只修改 0.15.26→0.15.27，500 项发布前回归、完整 TypeScript 检查和独立预检通过。09:09 原子推送 main 与 annotated tag，tag 对象 `da9484b1a83f8e2b0537275a31fb067883612751` 始终指向固定源码，没有重打 tag 或强推。

[run 40984099110](https://github.com/qinsehm1128/termul-new/actions/runs/40984099110) attempt 1 成功，09:30:53 +08:00 公开发布。9 个必需作业及三平台 native VT 检查通过，12 项公开资产、6 个 updater 平台键及公开 manifest/checksum 验收通过，安装前再次刷新验收。

远程 CI 不产出真实 Canary，因此在独立 scratch 源快照/target 内另行构建同源码别名包，而不是把正式 `.app` 改名：

- `SE_CANARY_BUILD=1`、`VITE_SE_CANARY=1`、`VITE_APP_VERSION_OVERRIDE=0.15.27-canary.1`。
- `CARGO_BUILD_JOBS=2`，独立 target、TMPDIR 与 Zig caches。
- 生产配置保留原生 Ghostty，后合并 Canary 配置和本地 override。
- `plugins.updater.endpoints=[]`、`bundle.createUpdaterArtifacts=false`。
- 构建环境采用允许列表，未传递 Apple/TAURI signing secret。Tauri `--no-sign` 后仅以 `-` 做 ad-hoc 签名。
- 所有四个 Mach-O 程序均为 arm64、ad-hoc 签名；bundle 严格签名验证通过，最终主程序含 Ghostty 符号，native archive 的 12 个 Mach-O 对象被动链接检查通过。

### 源码验证器问题已闭合

实际归档和产物在独立人工检查中是正确的，但交付的初版 `verify-build.py` 只比较工作快照与 producer claim，未强制归档 Git blob 和唯一锁文件字节变化。其旧 PASS 未被用于批准安装。

加固后的验证器绑定固定 commit/tag/tree Git 对象，核对归档完整覆盖、重复/特殊成员和源路径安全，逐文件重新计算 archive Git blob 并比对真实 Git OID及 producer claim，验证工作文件摘要/大小/执行位，并强制唯一变化为 `src-tauri/Cargo.lock` 根包 0.15.24→0.15.27 的精确单行替换。依赖字节不可变化。

- 2,114 个真实 tracked 文件被动验证通过。
- 7 项正负例回归通过，包括额外源码修改、依赖修改、同时改写声明的归档篡改、重复/缺失条目及错误变更声明。
- 父会话默认 Python 缺少 pytest，首次测试入口不可用。随后使用本次 scratch 下的隔离 uv/pytest 8.4.2 环境实际跑过 7 项测试，没有更改全局 Python 或弱化测试。
- 原独立 reviewer 复审 APPROVED，0 findings。最终 verifier SHA-256 为 `dfe125cc5a1437ce66b6344d646793f31f75c2a14921a135b215d3a0c7fe9ec5`。

### 安装安全与保护范围

原样复用了 7 个经审查的安装/归档工具，在新目录重新捕获 baseline，没有复制旧 baseline、receipt 或历史批准。40 项 dummy 工具测试由父会话和独立复核通过。本次还记录了两个受保护 `/Applications` bundle 的完整树，补足旧 helper 只校验选定文件的范围。

安装前新 source/旧 target 身份、arm64、签名、全树、无 Canary 进程/3292/socket owner、权限及容量检查通过。保守空间需求约 2.813 GB（含归档上限与 2 GiB 余量），检查时可用约 169 GB。11:02 冻结全部本次批准输入后，父会话才执行 apply。

- 仅替换 home Canary，使用 macOS 原子交换，原 app 保留在 `/Users/qs/Applications/.se-canary-install-6n6yvuvs/original-canary.app`。
- 完整备份为旧 app 物理副本和 manifest format 2 的无损持久数据归档，保留原始 xattrs，不忽略 provenance。只排除并记录两个临时 IPC socket。
- 独立 verifier 返回 `backup_verified=true`、`persistence_unchanged=true`、`stable_unchanged=true`、`protected_processes_unchanged=true`、`applications_launched=false`、`signals_sent=false`。
- Loop `l405` 已取消，不再重复构建或安装。

## 证据

证据根：`/Users/qs/.qin-code/scratch/root-drop-release-alias-0n4eep6o`。

| 证据 | 用途 |
| --- | --- |
| `initial-readonly-inventory.json`、`baseline.json` | 08:56 实际清单与09:06新基线，未重新捕获以掩盖变化 |
| `protected-app-trees.json`、`protected-check-*.json` | 正式版/旧系统 Canary 全树与受保护进程前后检查 |
| `publication-acceptance.json` | 公开 release、tag/source、CI、manifest和checksum验收 |
| `build/build-command.json`、`build/build-result.json` | 真实Canary参数、隔离环境、版本身份及构建结果 |
| `build/source-provenance.json`、`build/source-archive.tar` | 精确源码与唯一锁文件变更证据 |
| `parent-build-verification.json`、`source-verifier-tests-isolated.log` | 父会话实际来源验证与7项测试 |
| `helper-reuse-evidence.json`、`helper-tests.log` | 工具原样复用及本轮40项dummy测试 |
| `approved-install-inputs.json` | 本次最终批准哈希、目标、发布/产物门禁与安装完成记录 |
| `preinstall-validation-final.json`、`capacity-and-source-check.json` | 安装前只读预检和容量 |
| `backup-v0.15.27-canary.1/` | 已验证的完整备份 |
| `install-receipt-v0.15.27-canary.1.json` | 原子替换、原app保留路径与manifest摘要 |
| `installed-verification.json`、`final-install-acceptance.json` | 独立安装后验证及11:12:56最终验收 |

关键 SHA-256：

- 本次 baseline：`a64665ad9ac15186d43e34220469fc4681a571487a669e6727b85cd23e54297e`。
- 已安装 Canary 主程序：`682a7f7b8d030ccf9f2bb2ced1c247180cbaade50a6ac48c6cb6ff3fa11f1d48`。
- 已安装完整树：`64baabc98b8db848bb7a5d8d44ff4d55eaa3f5ae39350140d56e2407015cb11a`。
- 保留旧 Canary 完整树：`f65bce4a30324a5ffc2e5d1aaebffff765b3fe7ab1f794570d51dd9033c92025`。
- 备份 manifest：`e312f437919af4695c1e8c2b2255c238819af74ec5224ca6cac9b90c3df9e167`。
- 未变的正式版完整树：`556d73d68ce7a3a8d5caa924e645c44d07b16c2221d272624d8318bad178577e`。
- 未变的旧系统 Canary 完整树：`faf14f19db649157dccafcc6f73fe3e4d6c5bb3d9a71c9c9c81ba1ffe7a8d6c2`。

## 残留风险

- 未启动新 Canary，因此没有本轮安装后的 GUI、数据迁移、端口实际绑定或 TCC 授权验收。不能由静态签名和文件树通过推断这些已实测。
- 别名是 ad-hoc 签名，不是 Developer ID/notarized。公开更新签名未在本机独立密码学重验，远程二进制摘要以公开 checksum/API/CI 验证，不冒充全部二进制的本机字节重算。
- GitHub immutable release 模式未启用，本次 tag 未移动不等于未来不可被其他授权者修改。
- home 和系统目录旧 Canary 同 identifier。后续应从明确的 `~/Applications/Se Manager Canary.app` 路径打开，避免按 bundle ID 或 Spotlight 选错旧副本；系统旧副本本轮原样保留。
- Canary 的关键数据身份与正式版独立，但不是完整 OS 沙箱。SSH known-hosts 等仍有共享约定，两个应用打开同一物理项目时文件也共享。
- 归档无损保存原始元数据，不等于证明当前受跟踪进程能精确恢复缺失 provenance 属性。恢复必须重新验证，首次启动发生迁移后也不能盲目只降级 app。
- 没有当前安装阻塞。保留备份和原 app，不自动清理；原有用户未跟踪文件未纳入本次文档提交。
