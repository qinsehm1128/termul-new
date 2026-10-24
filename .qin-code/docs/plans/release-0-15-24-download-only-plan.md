---
schema: qin-doc/1
kind: plan
id: release-0-15-24-download-only
title: Se Manager v0.15.24完整发版与安装包下载
date: 2026-10-24
status: draft
source: user:2026-10-24T11:42:55+08:00; agent://release-workflow-preflight; agent://release-regression-check
lang: zh-CN
---

## 目标

按用户2026-10-24 11:42:55 +08:00请求：完整正式发版，然后将安装包下载到本机，先不要安装。

发布目标为新版本`v0.15.24`，包含`v0.15.23`之后的新建项目终端/屏幕观察引擎及gitignore选项、鼠标模式恢复修复、活动文件与文件树联动。下载本机适用的macOS Apple Silicon DMG。

## 非目标

- 不关闭现有正式版或Canary，不重启核心，不向PTY写入任何内容。
- 不挂载后执行安装，不替换`/Applications`程序，不启动新包。
- 不覆盖已有tag、不force push，不跳过正式工作流门禁，不部署或重启VPS。
- 不修订签名凭证或可选Homebrew配置。原安装Goal保持暂停，不复用其安装验收。

## 任务

1. 已核验：远端`main`是本地祖先，本地领先7个提交；最新已发布正式版为`v0.15.23`，新`v0.15.24`远端不存在。旧`v0.15.22`草稿保持不变。
2. 同步`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`和Cargo.lock根包版本为0.15.24，不改依赖版本。Bun锁文件无应用版本字段。
3. 回归与发布契约检查完成后提交版本准备，再推送main及唯一新tag，使用tag push触发Release。不得误用workflow_dispatch，其用途仅为已有Insider向stable晋升。
4. 跟踪Windows x64、macOS arm64、Linux x64 standalone server和最终publish/checksum任务，失败须查因修复，不以单平台成功宣称完整发版。
5. 核验非draft、非prerelease的正式Release，以及12个预期资产和公开latest/stable更新清单。验证componentPolicy使用canonical build ID并对应本次源提交。
6. 下载`Se.Manager_0.15.24_aarch64.dmg`及SHA256SUMS/更新清单到`~/Downloads/se-manager-v0.15.24/`，下载不覆盖已有文件，校验大小、GitHub digest、SHA256SUMS与DMG内部校验。不安装。
7. 保存证据并报告Release URL、版本、下载路径、校验结果及任何签名/验证限制。

### 已核验环境

- 本机`uname -m`为arm64。
- `HOMEBREW_TAP`未配置，可选cask的Intel包要求不构成本次门禁。不修改此配置。
- 已配置MACOS_CODESIGN和TAURI_SIGNING凭据名称，未配置APPLE Developer ID凭据名称。本轮沿用仓库既有自签名路径，不能宣称Apple公证通过。未读取或输出任何秘密值。
- 本轮scratch：`/Users/qs/.qin-code/scratch/se-release-0.15.24-KbjtpqkS`。
- 既有未跟踪mouse安装audit、knowledge、reference lock、release pycache不混入提交。

## 验收

- 预发布功能回归：19测试文件/537测试通过；node/web/test完整类型检查通过；se-pty 228通过、2个原有native-host集成测试因无生产宿主环境变量忽略。完整日志保留。正式CI仍执行自己的native Ghostty门禁。
- 版本一致性与发布脚本/工具链测试通过，提交和新tag的SHA一致，包含`e142a606`及`1b23efe5`。
- 正式Release与必要CI任务完成且成功。签名资产、12个发布文件、校验清单和latest/stable清单齐全，组件身份可追溯。
- 下载的arm64 DMG SHA256同时匹配GitHub资产digest与SHA256SUMS，DMG校验通过。
- 本轮没有执行应用关闭、安装、替换或启动新版本。
