---
schema: qin-doc/1
kind: plan
id: ghostty-release-publication
title: Ghostty VT 正式发布进度与失败修复计划
date: 2026-10-23
status: draft
source: user:2026-10-23T15:13:55+08:00; github-run:39917645711; github-run:39919761413
updated: 2026-10-23
lang: zh-CN
---

## 目标

按用户 2026-10-23 15:13:55 +08:00 请求，发布当前已验收的 Ghostty VT 版本及正式 tag。整个过程不在本机安装或启动新安装包。

既有上线流程是 GitHub 正式 Release 和桌面/server 更新清单，不包含未指定 VPS 的手动部署或重启。发布产物为 Windows x64、macOS arm64、Linux standalone `se-server`，不增加 Intel Mac/Linux 桌面目标。

## 非目标

- 不本地安装、不执行应用更新/重启，不运行安装脚本。
- 不未经指定部署或重启 VPS。
- 不强推、覆盖或删除已发布 tag，不绕过签名/native/产物审计门禁。

## 任务

### 已完成

- 初次发布前确认远端 latest 为 `v0.15.21`，main 无新增远端提交，`v0.15.22` 尚不存在。
- 四个版本文件提升为 0.15.22，锁文件精确同步。版本对齐、三个 TypeScript 工程、发布与 parity 测试通过。
- 提交 `f66fb55fd16d5726b3af47031e71a48ac9b636d6`。
- 15:25:38 +08:00 原子推送 main 和 annotated tag `v0.15.22`。远端 tag 对象 `bc0bc9c86ee7941db67271d2aaaa1683aa8de513`，peel 正确指向 f66fb55f。
- Release run `39917645711` 已 completed/failure，仅 Windows passive-link 审计失败。macOS 与 Linux server 构建成功。Release 保持 draft，线上仍为 `v0.15.21`，不能声称上线成功。
- 父会话新增手动远端 `.github/workflows/ghostty-native-preflight.yml`，提交并推送 `50495873397f416d2148f3bf4ef3bd686642f499`。
- Windows 诊断预检 `39919761413` completed/failure，真实 native VT 测试通过，passive-link 审计失败。取得真实静态库诊断 artifact，未下载或执行安装包。

### 当前阻塞与修复

Windows job `120295045359` 已通过真实 native VT 测试，但在 `Verify passive Ghostty native linkage` 被拒绝：

```text
FAIL passive-link audit: ...\Windows Kits\10\Lib\10.0.26100.0\um\x64\ntdll.lib:
unsupported object format (magic 213c6172)
```

这是上游组合静态库中嵌套的 ar archive（`!<ar`），不是 native 测试失败。失败关闭的门禁保持原样，不将未知 member 直接忽略。

- `release-windows-archive-fix` 独占两个被动链接审计 Python 文件，实现有界递归 archive 与严格必要的 COFF import 审计及回归。
- 真实产物：`/Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/windows-native-39919761413/se-ghostty-vt-2898f66b4479c1fd/out/native/lib/ghostty-vt-static.lib`，已交给该 worker 回归。
- worker 完成后，父会话复验真实 Windows 产物及测试，并独立审查。只提交本任务改动，不触碰既有 knowledge/reference lock 文件。
- 推送修复到 main 后触发手动 Windows native preflight。只有远端 native 与被动链接门禁通过，才准备新补丁版本 `v0.15.23`。不移动已有 `v0.15.22`。
- 首个 `gh run watch` 与 HTTP/1.1 watcher 均曾因 EOF 退出，这只说明本地监听失败，不代表远端 CI 状态。改用低频 loop 和 `GODEBUG=http2client=0 gh` 的单次查询核实。
- 发布成功后更新 Goal 为实际版本、commit、run，执行只读验收并取消 loop `l396`。

## 验收

只读线上校验脚本：`/Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/verify_release.py`。当前 Goal acceptance 暂绑定 `v0.15.22`/f66fb55f/run39917645711，正式发布修复版后须明确更新为实际新版本、commit 和 run。

必须同时满足：

1. Release workflow completed/success，构建 commit 与发布 tag 精确一致。
2. 非 draft、非 prerelease 的正式 Release，`releases/latest` 指向该版本。
3. 桌面包、server、签名、SHA256SUMS 与两个更新 JSON 全部 uploaded 且非空。
4. 公共 latest/stable 更新 JSON 版本正确，每个平台带非空签名，下载 URL 指向当前 tag 的真实资产。
5. 不执行本地安装，也不将下载 metadata/静态诊断库误称为安装。

可观察入口：GitHub Actions run 页面、Release tag 页面，以及 `https://github.com/qinsehm1128/termul-new/releases/latest/download/latest.json`。
