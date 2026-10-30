---
schema: qin-doc/1
kind: plan
id: root-drop-release-alias
title: 根目录拖放修复发布与独立Canary别名安装计划
date: 2026-10-30
status: draft
source: user:2026-10-30T08:52:57+08:00; commit:8b4f1ed1; github-run:40984099110; file:/Users/qs/.qin-code/scratch/root-drop-release-alias-0n4eep6o/final-install-acceptance.json
updated: 2026-10-30
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-alias-install"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-alias-prepare"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-preflight"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-release-publish"}],"goal_refs":[]}
---

## 目标

按用户 2026-10-30 08:52:57 +08:00 授权，发布根目录拖放修复，并安装不替换运行中 Se 的独立别名版本。**本计划于 2026-10-30 11:12:56 +08:00 完成最终验收。**

- 正式发布 `v0.15.27`，固定源码 `8b4f1ed1ee1a95de4288e81312cb525d744abe7e`，含修复 `0d48d768`。
- 本地目标仅 `~/Applications/Se Manager Canary.app`，由 `0.15.25-canary.1` 升级到同源码独立构建的 `0.15.27-canary.1`。
- 使用真实独立 identifier `com.se-manager.app.canary` 与 native/frontend Canary 构建标志，不是将正式版改名。

## 非目标

- 不替换、关闭、重启或向 `/Applications/Se Manager.app` 及核心进程发信号，不向活 PTY 输入命令。
- 不修改或删除系统目录的旧 `/Applications/Se Manager Canary.app` 0.12.3。
- 不自动启动新 Canary，不宣称运行时迁移、TCC 授权或新安装 GUI smoke test 已完成。
- 不改动 stable/Canary 的持久数据。旧 app 与完整无损持久数据归档均保留。
- 不移动 tag、强推、绕过 CI 或安装安全检查。别名包 updater endpoints/artifacts 关闭，不使用私钥签名，不宣称 notarized。

## 任务

1. **完成**：核对实际运行的正式版 0.15.24，主 PID 8769、核心 8849/8855/8857，以及空闲的 home Canary。固定目标为用户目录别名，两个 `/Applications` 应用不动。
2. **完成**：500 项发布前测试、完整类型检查和独立发布预检通过。版本三源更新并提交 `8b4f1ed1`，原子推送 main 与 annotated tag `v0.15.27`，tag 对象 `da9484b1a83f8e2b0537275a31fb067883612751`。
3. **完成**：[run 40984099110](https://github.com/qinsehm1128/termul-new/actions/runs/40984099110) 首轮成功，[Release v0.15.27](https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.27) 于 09:30:53 +08:00 公开。必需 CI、资产、manifest 和 checksum 验收通过，安装前再次刷新确认。
4. **完成**：在独立 scratch 源快照/target 构建真正的 Canary。限定 CARGO_BUILD_JOBS=2，保留原生 Ghostty，ad-hoc 签名，关闭 updater，不执行产品二进制。
5. **完成**：原样复用 7 个安装/归档 helper，重新捕获本次专用 baseline，未复制旧 baseline/receipt/审批。父与独立复核的 40 项 dummy 测试通过，正式版和旧系统 Canary 另有完整树基线。
6. **完成**：修复产物验证器的归档 Git blob 与唯一锁文件变更强断言缺口。7 项正负例测试、真实 2,114 文件被动复验和独立最终复审通过，随后冻结所有本次批准输入。
7. **完成**：11:04:40 +08:00 完成备份和原子交换，11:04:59 独立安装 verifier 通过，11:12:56 父再次确认已安装版本、完整树、备份、数据及保护对象不变。
8. **完成**：未启动任何 Se/Canary，没有发送应用进程信号。Loop `l405` 已取消，保留全部回滚与验收材料。

### 证据与回滚材料

证据根：`/Users/qs/.qin-code/scratch/root-drop-release-alias-0n4eep6o`。

- `final-install-acceptance.json`、`installed-verification.json`：最终验收通过。
- `approved-install-inputs.json`：冻结输入及实际安装状态。
- `backup-v0.15.27-canary.1/`：旧 app 物理备份与完整持久数据归档。
- `install-receipt-v0.15.27-canary.1.json`：原子交换与摘要记录。
- 原 app 保留在 `/Users/qs/Applications/.se-canary-install-6n6yvuvs/original-canary.app`。
- 本次 baseline SHA-256：`a64665ad9ac15186d43e34220469fc4681a571487a669e6727b85cd23e54297e`，从未为迁就后续状态而重捕。

## 验收

- [x] 公开 `v0.15.27`、所有必需 CI 和公开产物验证通过，原 tag/commit 未移动。
- [x] 别名来源精确绑定发布源码，arm64、ad-hoc 签名、原生链接、独立身份和 updater 禁用通过。
- [x] 新基线/脚本/产物哈希冻结，最后只读 preflight、磁盘容量及完整备份通过。
- [x] 仅 home Canary 被替换，已安装完整树与验收来源一致，持久数据未变。
- [x] 正式版与旧系统 Canary 完整树、主/core PID/PPID/启动时间/路径不变。
- [x] 未自动启动、关闭、重启应用，没有进程信号或 PTY 输入。
- [x] [最终审计](../audits/root-drop-release-alias-audit.md)明确记录尚未运行的检查与恢复限制。

剩余边界：Canary 不是完整 OS 沙箱，SSH known-hosts 仍有共享约定；若两个应用打开同一物理项目目录，用户之后的文件操作也会共享该目录。系统中存在旧同名 Canary，后续应从明确的用户目录路径打开，不按 bundle ID 或 Spotlight 猜测目标。归档保存原始元数据不等于证明当前进程能精确物化缺失 provenance 属性，恢复仍需独立检查。
