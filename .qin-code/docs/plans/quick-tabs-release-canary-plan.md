---
schema: qin-doc/1
kind: plan
id: quick-tabs-release-canary
title: 快捷多标签远程发布与Canary安全替换计划
date: 2026-10-26
status: draft
source: user:2026-10-26T16:57:57+08:00; commit:45f48a0b; github-run:40297303973; file:/Users/qs/.qin-code/scratch/quick-tabs-canary-release-IVm8yCR4/final-install-acceptance.json
updated: 2026-10-26
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-install"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-preflight"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-remote"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-verify"}],"goal_refs":[]}
---

## 目标

按用户 2026-10-26 16:57:57 +08:00 授权，远程发布快捷终端多标签功能，然后仅替换本地 `~/Applications/Se Manager Canary.app`，不触碰主进程。**本计划于 19:26:09 +08:00 完成验收。**

- 正式发布：`v0.15.25`，源码 `45f48a0b6b08483f875c46accb0455500985eb3a`。
- 同源本地 Canary：`0.15.25-canary.1`，身份 `com.se-manager.app.canary`，不是将正式版或 Nightly 改名。

## 非目标

- 不修改、关闭、重启或替换 `/Applications/Se Manager.app`，不向活 PTY 注入输入或发送进程信号。
- 不触碰 `/Applications/Se Manager Canary.app` 的旧 0.12.3 副本，不自动启动新版 Canary。
- 不覆盖现有正式 tag，不强推，不跳过 CI、签名或架构校验。
- 不创建 Canary 更新通道，不读取/生成私钥。本地 Canary 使用独立 ad-hoc 签名，关闭 updater endpoints/artifacts。
- 不修改正式版或 Canary 持久数据。保留完整旧 app 和已校验数据归档。

## 任务

1. **完成**：确认远端只有 stable/nightly CI，nightly 是正式版身份。选用既有 stable workflow 发布，再从同一源码构建真正 Canary。
2. **完成**：package.json、Cargo.toml、tauri.conf.json 对齐 0.15.25，25 项相关测试及提交钩子类型检查通过。17:08 +08:00 原子推送 main 与新 tag。
3. **完成**：[run 40297303973](https://github.com/qinsehm1128/termul-new/actions/runs/40297303973) attempt 2 成功，18:17:43 +08:00 公开 [v0.15.25](https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.25)。首次 Linux Checkout 失败且收尾卡住，正常取消后定向重跑，未改 tag 或安全门禁。
4. **完成**：独立公开发布核验 29 项通过。实际下载 macOS DMG/tar 和 manifests/checksums，并重算 6 个下载文件的 SHA-256。来源通过 tag、运行提交和 component identity 绑定。
5. **完成**：独立构建并核验 Canary 精确来源、arm64、ad-hoc codesign、native/frontend Canary 标志、Ghostty 和完整 app 树。
6. **完成**：第一次 apply 因 WebKit 备份新增 OS 来源属性在交换前停止。根因调查后采用[无损归档决策](../decisions/2026-10-26-canary-persistence-archive.md)，持久数据原始属性不忽略，app 仍严格物理树相等。40 项 dummy tests 和独立最终审查通过，0 findings。
7. **完成**：19:25:22 重新冻结获批哈希并通过真实只读预检。19:25:38 以全新备份路径完成原子替换。19:25:48 独立 verifier 通过，19:26:09 父会话再验主程序/核心进程和两个受保护系统目录 app 未变。

### 证据与保留材料

证据根：`/Users/qs/.qin-code/scratch/quick-tabs-canary-release-IVm8yCR4`。

- 完整备份：`backup-v0.15.25-canary.1-archive-v2`。
- 安装 receipt：`install-receipt-v0.15.25-canary.1-archive-v2.json`。
- 最终验收：`installed-verification-archive-v2.json` 与 `final-install-acceptance.json`。
- 原 app：`/Users/qs/Applications/.se-canary-install-iz0ubvml/original-canary.app`。
- 第一份失败部分备份保留为诊断，未复用。
- 正式版 0.15.24 主 PID `8769` 启动于 08:52:40 +08:00，核心 PID `8849`、`8855`、`8857` 启动于 08:52:43 +08:00，安装后与原始基线一致。
- 既有其他 audit、知识、锁文件和 pycache 保留，不纳入本任务提交。

## 验收

- [x] 所有必需远程检查通过，公开版本/目标资产/SHA256/updater manifests 和精确来源已核验。
- [x] Canary 版本/独立身份/arm64/完整签名及真实原生功能符合已验收产物。
- [x] 安装前后主 PID 与核心 PID 的启动时间、路径、PPID，以及正式版/旧系统 Canary 指纹不变。
- [x] 新 app 全树等于来源，保留旧 app 精确副本，持久数据归档完整且 live persistence 未变。
- [x] 未启动、关闭、重启或发送信号，未把无运行 smoke test 伪称为已验证。
- [x] 残留限制与全部失败/恢复路径见[最终验收审计](../audits/quick-tabs-release-canary-audit.md)。特别说明：归档保存精确原始元数据不等于证明当前进程可物化缺失来源属性；恢复遇到差异必须停止，不覆盖 live persistence。
