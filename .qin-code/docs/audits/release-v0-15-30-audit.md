---
schema: qin-doc/1
kind: audit
id: release-v0-15-30
title: v0.15.30发布：插件宿主能力、网页界面与后台进程
date: 2026-11-04
status: accepted
source: user:2026-11-04T15:51:35+08:00; commit:23bb4a69; github-run:41795274878
lang: zh-CN
scope: project
---

## 结论

v0.15.30 已公开发布并标记为 Latest。这一版在 v0.15.29 的原生插件系统上补齐了宿主能力和界面扩展：

- 侧栏视图，以及插件管理页（列表、详情、设置、扩展能力、服务、进程）；
- 可折叠的活动栏分组；
- 宿主上下文与事件、键值存储、通知与进度、四类右键菜单、快捷键、状态栏项；
- `.seplugin` 插件包，以及在隔离 iframe 中运行的插件网页界面（`se-plugin` 协议）；
- 后台唤醒（`se.host.wake`）和受管后台进程（`contributes.processes`）；
- 示例插件 Activity Studio。

各项链接如下：

- 发布：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.30
- 源提交：`23bb4a69fe8cde238833b668152bc23f4e818919`（chore: bump release version to 0.15.30）
- 标签：附注标签 `v0.15.30`，对象 `0fe74706`，信息 "Se Manager v0.15.30"
- 推送：origin/main `7df42993..23bb4a69`。本次共 26 个提交，从 `690a0b48` 到 `23bb4a69`。

## 发现

1. Release 工作流 `41795274878` 结论为 success，用时约 25 分钟（07:52:23Z → 08:16:56Z）。以下各项都成功：
   - 三个平台构建和 se-server；
   - 校验并发布；
   - SHA256SUMS。
   
   Homebrew cask 和 Insider 提升按工作流条件跳过，和 v0.15.29 一致。
2. 提示性步骤「Explain component identity changes」这次正常输出：renderer、guiNative、acpCore、terminalCore、pluginCore 五个组件都是 changed。v0.15.29 审计里记录的 `missing pluginCore` 没有再出现。
3. `latest.json` 保持向后兼容：`termul.componentPolicy.components` 仍是原来 4 个组件。pluginCore 是同级字段，buildId 为 `termul-pluginCore-v1-sha256:745f0f01…`，action 为 `defer-if-active`。
4. 发布前的隔离 GUI 实测发现三个问题，都已修复并补了测试：
   - 网页视图入口少了 `ui/` 路径，导致 404（`47b6defe`）；
   - 前端仍按旧地址格式校验会话 URL（`225cb777`）；
   - 网页视图被当成声明式面板刷新，每次唤醒都记录 `PLUGIN_NOT_FOUND`（`eb5a0c93`）。
   
   这次实测由用户在 `com.se-manager.app.plugintest` 隔离实例里手动完成，cua-driver 只做了前半段。
5. 独立审查（cebff97f..98215542）结论为 ship-after-fixes，两个问题已在 `e3f004c6` 修复：
   - RV-001：进程经 exec 换了程序后，孤儿进程回收会漏掉；
   - RV-002：上下文重置时，网页订阅收不到重新同步通知。
   
   此前 W5 端到端测试暴露的两个宿主缺陷在 `ea85b39f` 修复：网页读取会清空原生事件队列，网页事件格式和 shim 不匹配。同一提交还为受管进程加了持久清单和启动回收。

## 证据

- `gh run view 41795274878`：completed success。Generate Changelog、Generate component identity、Create Draft Release、Build (macos-aarch64)、Build (windows-x64)、Build se-server (linux-x64)、Validate upload and publish release、Resolve release metadata、Generate SHA256SUMS 均为 success，没有失败步骤。
- `gh release view v0.15.30`：draft=false、prerelease=false，发布时间 2026-11-04T08:16:17Z，共 12 个资产：
  - `latest.json`、`latest-stable.json`
  - `Se.Manager_0.15.30_aarch64.dmg`、`Se.Manager_aarch64.app.tar.gz(.sig)`
  - `Se.Manager_0.15.30_x64-setup.exe(.sig)`、`Se.Manager_0.15.30_x64_en-US.msi(.sig)`
  - `se-server(.sig)`、`SHA256SUMS.txt`
- `gh release list`：v0.15.30 为 Latest。
- `latest.json`：version 0.15.30。平台为 darwin-aarch64、darwin-aarch64-app、linux-x86_64-server、windows-x86_64、windows-x86_64-msi、windows-x86_64-nsis。
- 发布前本地验收：
  - 整个工作区 `cargo check --all-targets` 和 `cargo test --no-run` 通过。
  - 测试：se-manager 插件 40 个、协议 14+14+7 个、插件宿主单测 132 个全部通过。
  - 真实插件运行器 `scripts/test-plugins.sh` 通过：打包工具 1、网页界面端到端 2、原生端到端 7、Plugin Core 冒烟 1、前端 72。运行后没有残留的服务或 Core 进程。
  - 插件 crate 的 strict clippy 无警告，三套 TypeScript 类型检查通过。
  - 前端全量测试有 82 个失败，和基线 `fail-rail.txt` 逐条一致，没有新增。
- 隔离实测结束后，测试实例的进程已停止，数据目录、日志和 `mcp-gateway.dev.json` 按用户选择删除。用户正在使用的 Se 进程 PID 列表和测试前的基线一致。

## 残留风险

- 插件系统只在 macOS 上实际运行过。Windows 和 Linux 只有 CI 编译通过。Windows 上停止后台进程是尽力而为，没有用 job object，进程可能关不干净。
- 受管进程用 Unix 进程组管理，这不是沙箱。如果插件进程自己脱离了进程组，停用或回收时不保证能一起结束。原生插件以当前用户的全部权限运行，这是设计上接受的。
- 插件网页的 iframe 隔离（拿不到 Tauri IPC、读不到父窗口 DOM、origin 为 opaque）有两方面依据：静态审查和 jsdom 测试；实测中网页视图能正常工作。但实测时没有逐项记录「Sandbox check」卡片的显示值，WKWebView 和 WebView2 上的隔离还需要专门确认。
- Plugin Core 日志每 3 秒出现一次 `plugin_connection_closed reason=transport_failure`（`src-tauri/src/core/plugin.rs:395`），时间和健康检查探测一致，判断是误报的日志，但没有修复，也没有证实。
- Rust 侧的 PluginCoreClient 还没有实现序号缺口后的重新同步。
- 网页会话的重复请求账本上限为 4096 条，用尽后需要重新打开会话。
- 工作流提示：部分 Action 目标是 Node.js 20，被强制在 Node.js 24 上运行。目前不影响发布，以后需要升级这些 Action 的版本。
