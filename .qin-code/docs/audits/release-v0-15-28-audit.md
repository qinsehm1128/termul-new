---
schema: qin-doc/1
kind: audit
id: release-v0-15-28
title: v0.15.28发布：鼠标键盘接管同步与快捷终端标签配色
date: 2026-11-03
status: accepted
source: user:2026-11-03T12:16:13+08:00; commit:4397faf6; github-run:41577858560
lang: zh-CN
scope: project
---

## 结论

**v0.15.28 已正式发布并成为 latest。** Release：<https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.28>，源码 `4397faf6ad5e2499ca21076074393460793c2e66`，annotated tag 对象 `d7034a7b`（"Se Manager v0.15.28"）。

包含的改动：

- `84789df5` 宿主接口 `terminal_input_modes`（se-pty `InputModes`、core RPC `terminal.input-modes`、Tauri 命令），renderer 重新 attach 后自动按宿主模式恢复鼠标键盘接管，「宿主屏幕」面板新增「重新同步鼠标键盘接管」按钮。只写本地 xterm，不向 PTY 发送任何字节。
- `47b54c43`、`864f4fff` 快捷终端标签改用与工作区标签栏共用的 `tab-tone.ts`，只用主题变量，跟随偏好设置的颜色主题。
- `4397faf6` 版本三源 0.15.27→0.15.28，`Cargo.lock` 只改 se-manager 版本行。

## 发现

- [run 41577858560](https://github.com/qinsehm1128/termul-new/actions/runs/41577858560) attempt 1 成功（12:23:36→12:45:24 +08:00），9 个作业成功，2 个按设计跳过（Promote Insider、Homebrew cask）。
- 12:44:48 +08:00 公开发布，非 draft、非 prerelease，12 项资产：macOS aarch64 dmg 与 app.tar.gz、Windows msi 与 nsis、se-server，各自 `.sig`，`latest.json`、`latest-stable.json`、`SHA256SUMS.txt`。
- `latest.json` 版本 0.15.28，6 个 updater 平台键全部指向 v0.15.28 资产。
- `e7475bc0 base`（作者 `test <test@test>`）是空提交，随本次推送进入 main，不含文件改动。

## 证据

- 发布前：tsc node/web/test 通过；`cargo test -p se-pty -p se-terminal-core` 229 + 34 通过。
- 全量 vitest：HEAD 82 项失败，v0.15.27 基线 87 项失败；逐项对比，HEAD 没有基线之外的新失败。失败集中在 `ConnectedTerminal.test.tsx`（79）、`env-name-parity.test.ts`（2）、`local-terminal-spawns.test.ts`（1），均为既有问题。
- 新增测试：真实 xterm 下接管恢复与释放 5 项、面板 11 项、quick-terminal 与 workspace 共 192 项全部通过。

## 残留风险

- 鼠标接管同步与标签配色没有在安装后的应用界面里手动验证。
- 本机 `/Applications/Se Manager.app` 未更新，需通过应用内更新或手动安装 0.15.28。
- `ConnectedTerminal.test.tsx` 的既有失败会掩盖该组件未来的回归，需要单独修。
- 工作区存在他人未提交的 `.gitignore` 改动 `/.qin-code/`，会让新增的 `.qin-code/docs` 文件默认被忽略，未纳入本次提交。
