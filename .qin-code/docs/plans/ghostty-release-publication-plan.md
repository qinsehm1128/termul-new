---
schema: qin-doc/1
kind: plan
id: ghostty-release-publication
title: Ghostty VT 正式发布进度与失败修复计划
date: 2026-10-23
status: draft
source: user:2026-10-23T15:13:55+08:00; user:2026-10-23T16:49:36+08:00; commit:6d8354a4; agent://release-windows-audit-review
updated: 2026-10-23
lang: zh-CN
---

## 目标

发布已验收的 Ghostty VT 修复版本 `v0.15.23`，提供 Windows x64、macOS arm64、Linux standalone `se-server` 的 GitHub Release 与签名更新清单。

用户原请求 2026-10-23 15:13:55 +08:00：发布上线及 tag，禁止本机安装。用户最新请求 16:49:36 +08:00：**Windows 可以先不检测**。据此跳过本次额外手动 Windows native preflight，不再将该预检作为打 tag 前置。正式 Release 既有构建、签名及被动链接审计不改弱。

## 非目标

- 不在本机安装或启动新包，不运行安装/更新脚本，不擅自部署或重启 VPS。
- 不强推、覆盖或删除已有 tag。`v0.15.22` 保持原样。
- 不绕过正式发布的签名、native 或产物完整性门禁。
- 不增加 Intel Mac/Linux 桌面目标。

## 任务

### 已完成

- 原版 `v0.15.22` / `f66fb55fd16d5726b3af47031e71a48ac9b636d6` 已于 15:25:38 +08:00 原子推送。tag 对象 `bc0bc9c86ee7941db67271d2aaaa1683aa8de513` peel 正确。
- Release `39917645711` completed/failure。macOS/Linux server 成功，仅 Windows passive audit 因嵌套 `ntdll.lib` ar 格式 `magic 213c6172` 失败。Windows native VT 测试通过。发布仍 draft，线上 latest 仍 `v0.15.21`。
- `50495873397f416d2148f3bf4ef3bd686642f499` 新增手动预检，诊断 run `39919761413` 重现问题，下载真实静态库而非安装包。
- `951d362871b1ddb787aba54301af7c7fedd52e48` 加入有界嵌套 ar 和严格必要的 COFF import 支持。父复验 Python3.11/3.12 各16测试、真实 Windows3627对象、macOS12对象通过。
- `ac1e2308bf744f85051818a39ced2b9111930f9c` 修复预检/正式/夜间工作流五个 passive 步骤的失败传播。显式 bash + `set -euo pipefail`。手动预检完整 expected_revision 必须与 HEAD/GITHUB_SHA一致，诊断缺失error。独立复核 approved。
- 发布/parity 共240项测试、三个 TypeScript 工程、Biome通过。parity既有 System/Keyboard/QuickTerminal 推荐测试缺失警告保留。
- `6d8354a48bab6375481bbfc7a052bff1bdec927d` 已提交0.15.23四文件版本，align、locked Cargo metadata及240测试通过，尚未创建新tag。
- 16:47 +08:00 推送因 LibreSSL SSL_ERROR_SYSCALL 失败，因此未触发手动预检。16:52 +08:00 使用 HTTP/1.1 的只读远端查询已恢复，远端 main 仍50495873，v0.15.23仍不存在。

### 当前定点收尾

独立 `release-windows-audit-review` 在951d3628发现：扩展COFF relocation count包含marker，但span少算一条，可能放过截断或借用符号表字节的成员。这是已明确的边界错误，不以跳过额外Windows检测为由保留错误。

- 父已补充 `coff_extended` helper、marker完整范围/符号表重叠/保留字段负向用例及逐字节截断用例。
- `release-coff-boundary-fix` 独占两份passive-link Python文件完成最小修复与Python3.11/3.12离线回归。不跑额外真实Windows产物检查、不触发手动预检、不构建或安装应用。
- 该修复交付后提交，只进行必要离线验收，即推送main与新annotated `v0.15.23`。不再等待手动Windows预检。
- 正式run出现后更新Goal到实际version/commit/run，恢复低频CI追踪，等待签名发布及线上清单验收。

### 证据与追踪

- 真实Windows静态库：`/Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/windows-native-39919761413/se-ghostty-vt-2898f66b4479c1fd/out/native/lib/ghostty-vt-static.lib`。
- macOS既有静态库：`/Users/qs/.qin-code/scratch/se-ghostty-vt-prebuilt-verified/lib/libghostty-vt.a`。
- 旧loop `l396` 已取消。`gh run watch` 的EOF仅为监听断开，不代表远端结果。单次查询使用 `GODEBUG=http2client=0 gh`，git必要时用HTTP/1.1。
- 既有knowledge/reference lock和生成的`__pycache__`不纳入任务提交。

## 验收

只读脚本：`/Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/verify_release.py`。新run出现后须替换Goal的旧v0.15.22失败run绑定。

1. 正式Release workflow completed/success，commit与tag一致。
2. 非draft、非prerelease，GitHub latest指向新版本。
3. 桌面包、server、签名、SHA256SUMS、两个更新JSON uploaded且非空。
4. 公共latest/stable版本正确，各平台有签名，下载URL指向当前tag真实资产。
5. 未本机安装/启动新包，未擅自部署或重启VPS。

公开入口为Actions run、Release tag页面与 `https://github.com/qinsehm1128/termul-new/releases/latest/download/latest.json`。
