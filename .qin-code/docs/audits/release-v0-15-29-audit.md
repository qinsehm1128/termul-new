---
schema: qin-doc/1
kind: audit
id: release-v0-15-29
title: v0.15.29发布：原生插件系统
date: 2026-11-03
status: accepted
source: user:2026-11-03T22:14:48+08:00; commit:c5a498d1; github-run:41651935574
lang: zh-CN
scope: project
---

## 结论

v0.15.29 已公开发布并标记为 Latest。这是第一个包含原生插件系统（C-ABI cdylib 插件、独立 Plugin Core 进程、插件管理页、VS Code 风格的插件设置、声明式插件面板）的版本，也是 pluginCore 组件身份第一次在 CI 中生成。

- 发布：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.29
- 源提交：`c5a498d128c39c7f69b036b7d304473ee4de3eb6`（chore: bump release version to 0.15.29）
- 标签：附注标签 `v0.15.29`，对象 `5efe6b2e`，信息 "Se Manager v0.15.29"
- 推送：origin/main `e83cf9f3..c5a498d1`（本次共 19 个提交，从 `58bd7b9e` 到 `c5a498d1`）

## 发现

1. Release 工作流 `41651935574` 结论为 success，用时约 27 分钟（14:15:41Z → 14:42:33Z）。三个平台构建、se-server、校验并发布、SHA256SUMS 全部成功。Homebrew cask 和 Insider 提升按工作流条件跳过，和 v0.15.28 一致。
2. 附带的提示性步骤「Explain component identity changes」（`continue-on-error: true`）失败，输出 `component identity inputs missing pluginCore`。原因：它用 `component-build-id.mjs --explain-against v0.15.28` 和上一个版本比对，而 v0.15.28 的 `component-build-inputs.json` 还没有 pluginCore。这一步只生成说明，不影响组件身份和发布产物；下一个版本对比的是 v0.15.29，两边都有 pluginCore，不会再出现。
3. `latest.json` 保持向后兼容：`termul.componentPolicy.components` 仍是原来 4 个组件（renderer、guiNative、acpCore、terminalCore），pluginCore 作为同级的新增字段，action 为 `defer-if-active`。

## 证据

- `gh run view 41651935574`：completed success；Generate Changelog、Generate component identity、Create Draft Release、Build (macos-aarch64)、Build (windows-x64)、Build se-server (linux-x64)、Validate upload and publish、Resolve release metadata、Generate SHA256SUMS 均为 success。
- `gh release view v0.15.29`：draft=false、prerelease=false、发布时间 2026-11-03T14:41:38Z，12 个资产：`latest.json`、`latest-stable.json`、`Se.Manager_0.15.29_aarch64.dmg`、`Se.Manager_aarch64.app.tar.gz(.sig)`、`Se.Manager_0.15.29_x64-setup.exe(.sig)`、`Se.Manager_0.15.29_x64_en-US.msi(.sig)`、`se-server(.sig)`、`SHA256SUMS.txt`。
- `gh release list`：v0.15.29 为 Latest。
- `latest.json`：version 0.15.29；平台为 darwin-aarch64、darwin-aarch64-app、linux-x86_64-server、windows-x86_64、windows-x86_64-msi、windows-x86_64-nsis；pluginCore buildId `termul-pluginCore-v1-sha256:0a1d1ca6…`。
- 发布前的本地验收：真实插件测试（端到端 4 个、Plugin Core 冒烟 1 个）通过；插件宿主单测 58 个、核心进程 138 个通过；插件相关 crate 和 se-manager 的 strict clippy 无警告；整个工作区 `cargo check` 通过；三套 TypeScript 类型检查通过；前端全量测试 82 个失败全部是 v0.15.28 起就存在的；在隔离测试实例里用 cua-driver 完整操作了安装、启用、设置、面板、升级、停用、卸载。
- 发布前做了三路深度审查（FFI、跨进程故障路径、安全面），发现 11 个问题，修复 10 个（提交 `44e6dbc3`、`9b2cfea7`、`a90ec177`、`a2ba5462`、`e7736261`、`ad9dc2b3`），剩下 1 个是旧注册表迁移，因为插件系统此前从未发布，不需要。

## 残留风险

- 已安装 0.15.28 的用户升级后，旧的 Terminal Core 按 defer-if-active 策略会继续运行到被替换为止；在此之前 `terminal.input-modes` 不可用（v0.15.28 遗留，已知）。
- Plugin Core 第一次随更新器下发，真实用户机器上的首次启动、更新替换没有在已安装版本上验证过，只在调试版隔离实例里验证。
- 原生插件以当前用户权限运行，没有沙箱，这是设计上接受的。macOS 正式版目前未开 Hardened Runtime，第三方插件 dylib 能加载；若以后开启或公证，需要重新评估。
- Windows 和 Linux 上插件系统没有运行验证，只有 CI 编译通过。
- 「Explain component identity changes」这一次的说明缺失（见发现 2）。
