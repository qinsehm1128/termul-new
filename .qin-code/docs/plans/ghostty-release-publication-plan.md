---
schema: qin-doc/1
kind: plan
id: ghostty-release-publication
title: Ghostty VT 正式发布进度与失败修复计划
date: 2026-10-23
status: draft
source: user:2026-10-23T15:13:55+08:00; github-run:39917645711; github-run:39919761413; commit:ac1e2308; commit:951d3628; agent://release-preflight-review
updated: 2026-10-23
lang: zh-CN
---

## 目标

按用户 2026-10-23 15:13:55 +08:00 请求，发布已验收的 Ghostty VT 版本及正式 tag。全程不在本机安装或启动新安装包，不擅自部署或重启 VPS。交付为 GitHub Release、桌面/server 更新清单，平台为 Windows x64、macOS arm64、Linux standalone `se-server`。

## 非目标

- 不执行本地安装、应用更新/重启、安装脚本或未指定 VPS 操作。
- 不强推、覆盖或删除已有 tag，不绕过签名/native/产物审计门禁。
- 不增加 Intel Mac/Linux 桌面目标。

## 任务

### 首次发布与诊断

- `f66fb55fd16d5726b3af47031e71a48ac9b636d6` 四个版本文件对齐 0.15.22，发布/类型检查通过。15:25:38 +08:00 原子推送 main 与 annotated `v0.15.22`。tag 对象 `bc0bc9c86ee7941db67271d2aaaa1683aa8de513` peel 正确。
- Release `39917645711` completed/failure，仅 Windows passive-link 审计失败，macOS/Linux server 成功。`v0.15.22` 保持 draft，线上仍 `v0.15.21`。
- 失败原因：组合静态库内 `ntdll.lib` 为嵌套 ar，旧审计器拒绝 `magic 213c6172`。Windows native VT 测试本身通过。
- `50495873397f416d2148f3bf4ef3bd686642f499` 新增手动远端预检。诊断 run `39919761413` 重现失败并下载真实静态库，未下载或执行安装包。

### 已完成的修复与验证

- `951d362871b1ddb787aba54301af7c7fedd52e48` 修复两份 passive-link Python 文件：有界递归 ar、严格必要 COFF import 验证、扩展 relocation 边界。未知或畸形成员继续拒绝。
- 父会话独立复验：Python3.11/3.12 各16测试通过，真实 Windows 3627 个 COFF/COFF-import 对象通过，既有 macOS 12 个 Mach-O64 对象通过。
- 真实 Windows 输入为 `/Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/windows-native-39919761413/se-ghostty-vt-2898f66b4479c1fd/out/native/lib/ghostty-vt-static.lib`。macOS 输入为 `/Users/qs/.qin-code/scratch/se-ghostty-vt-prebuilt-verified/lib/libghostty-vt.a`。
- 独立审查 `release-windows-audit-review` 正审查提交 951d3628。未收到批准前不打新 tag。
- 预检独立审查发现 PowerShell 可能覆盖前一条测试失败、手动预检未绑定不可变 SHA、诊断缺失仅告警。`ac1e2308bf744f85051818a39ced2b9111930f9c` 已修复，复审 approved。
- preflight/release/nightly 全部五个 passive self-test + audit 步骤显式 bash + `set -euo pipefail`。手动预检输入 `expected_revision` 为完整 SHA，checkout HEAD 与 workflow GITHUB_SHA 均须相同。诊断 artifact 缺失为 error。
- 新增9项真实执行工作流 shell 的失败传播和 SHA 漂移测试。全部发布/parity 240测试、三个 TypeScript 工程、Biome 通过。parity 既有 System/Keyboard/QuickTerminal 推荐测试缺失警告保持记录。
- 下一补丁版 0.15.23 的三个源码版本文件与精确根锁文件已准备，版本对齐、locked Cargo metadata、typecheck、240测试通过。尚未创建新 tag。

### 后续顺序与并行约束

1. 版本提交形成待发布最终 SHA，推送 main，以此完整 SHA 运行 `ghostty-native-preflight.yml --ref main -f expected_revision=<sha>`。
2. 远端预检可与独立代码审查并行，但两者均通过前禁止打 tag。有审查修复则新 commit、新 SHA 重新预检。
3. 核实预检 run head_sha 与完成状态，只有同 SHA 审查批准、native 和 passive 审计成功，才为该 SHA 创建新 annotated `v0.15.23`。不移动已有 `v0.15.22`。
4. 将 Goal acceptance 更新为实际 version/commit/release run，跟踪正式构建、签名、发布与公开清单。成功后结束发布任务并取消有效 CI loop。

旧 `gh run watch` 两次 EOF 仅代表本地监听断开。使用 `GODEBUG=http2client=0 gh` 单次查询。旧 loop `l396` 已在旧 run 终止时取消，触发新 CI 后恢复低频追踪。既有 knowledge/reference lock 和生成的 `__pycache__` 不纳入本次提交。

## 验收

只读脚本：`/Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/verify_release.py`。Goal 当前旧版本绑定必须在新发布 run 产生后更新，不能误验旧失败 run。

必须同时满足：

1. Release workflow completed/success，构建 commit 与 tag 精确一致。
2. 非 draft、非 prerelease，GitHub latest 指向新版本。
3. 桌面包、server、签名、SHA256SUMS、两个更新 JSON uploaded 且非空。
4. 公开 latest/stable 清单版本正确，各平台有签名，下载 URL 指向当前 tag 的真实资产。
5. 未在本机安装/启动新包，未擅自部署或重启 VPS。

公开入口为 Actions run、Release tag 页面及 `https://github.com/qinsehm1128/termul-new/releases/latest/download/latest.json`。
