---
schema: qin-doc/1
kind: audit
id: worktree-hover-v01526-release
title: v0.15.26 worktree悬浮提示远程发布验收
date: 2026-10-28
status: draft
source: user:2026-10-28T17:18:28+08:00; commit:8217aa37; github-run:40663980405; agent://hover-v01526-release-review; agent://release-v01526-windows-native-debug; file:/Users/qs/.qin-code/scratch/worktree-hover-release-v01526-74x_fp6y/publication-acceptance.json
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"hover-release-preflight"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"hover-release-trigger"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"hover-release-verify"}],"goal_refs":[]}
---

## 结论

**仅远程发布已完成。** 用户 2026-10-28 17:18:28 +08:00 要求发布 tag，不需要本地打包。本次没有执行本地构建/打包、应用安装、应用启动/停止/重启或进程信号操作。

- 正式版本：[v0.15.26](https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.26)。
- 发布源码：`8217aa37a7c8f436ed0c66cc21c0fbd39093c369`，包含 worktree 悬浮提示修复 `06a5c4da0de18d6ed6833f7acd56bdcbdb6e45dd`。
- annotated tag 对象：`207e4d7ee3d4671c884ec8a6badb13851920c67a`，从创建至最终验收始终指向原发布源码，没有覆盖 tag 或强推。
- [Release run 40663980405](https://github.com/qinsehm1128/termul-new/actions/runs/40663980405) attempt 2 成功，9 个必需作业及三平台原生 VT 门禁均通过。
- Release 于 **18:12:41 +08:00** 公开，18:17:58 +08:00 完成父会话只读公开产物验收，非 draft、非 prerelease。
- 12 项公开资产存在，6 个 updater 平台键完整，manifest 与校验和检查通过。等待 Loop `l404` 已取消。

## 发现

### 发布范围与前置检查

本次产品改动仅为 worktree 菜单悬浮展示完整名称、不同的分支名与路径，主目录也展示完整信息。不改变菜单布局、选择或终端行为。

- hover 修复及相关跨根回归共 265 项通过，独立 UI 审查 0 findings。
- 发布前重新执行 4 个文件、126 项发布契约/侧栏回归，全部通过。
- package.json、src-tauri/Cargo.toml、src-tauri/tauri.conf.json 仅 3 行版本从 0.15.25 升至 0.15.26，版本 guard 和提交钩子全部 TypeScript 检查通过。
- 独立发布预检批准，0 findings。没有更新依赖解析；远程 CI 按既有脚本只同步 Cargo.lock 根包版本。
- 17:24:26 +08:00 原子推送 main 与新 tag。随后发生的文件剪贴板调研文档提交 `0575800a` 留在本地，不改变已固定的发布源码。

### 首轮 Windows 失败与恢复

attempt 1 的 macOS 与 Linux 作业成功，Windows 在 `Verify real Ghostty native VT` 步骤失败。实际错误是 Zig 无法读取/解包固定的 uucode tarball，`build.zig.zon:51` 报 `ReadFailed`，由 `se-ghostty-vt/build.rs:106` 传播。原生测试当时尚未执行，不能称为测试断言失败或签名失败。

独立诊断确认：

- 与上一成功发布相比，相关 workflow、原生源码、工具脚本及 vendored 源归档没有变化。
- 同一依赖的单次公开下载返回 HTTP 200，2,214,904 字节，gzip 与 75 个 tar 条目可读取。
- 下载 SHA-256 为 `7e76fc7fab1e7ac728c52b35bbb3e5b8c639841abfc7fe1a4bcb13050594bc9e`。这不是 Zig package-content hash 的独立验证。
- 上次 v0.15.25 Windows 作业和对应原生检查成功。旧完整日志没有取得，不把元数据检查伪称为完整日志比较。

基于这些证据，于 17:52:33 +08:00 对 Windows job `122733865847` 及其后续依赖请求了一次定向重跑，未改源码、tag、签名或安全门禁。attempt 2 Windows job `122746181058` 于 17:59:28 通过 native VT、17:59:47 通过 passive linkage，之后远程构建、发布校验和公开上传全部成功。

底层网络流读取、Windows 临时文件/缓存 I/O 或 Zig 环境因素没有被唯一归因。相同源码重跑成功支持暂时性获取/读取故障，不等于已证明具体根因。

### 公开产物验收

只读验收脚本验证了 run/head、tag 对象与指向、公开 release 状态、所有资产上传状态、9 个必需作业及三平台 native VT 成功。

匿名下载 `SHA256SUMS.txt`、`latest-stable.json` 和 `latest.json`，重算实际字节 SHA-256 并与 GitHub asset digest 一致。两个更新清单逐字节相同。校验和中 6 项记录与 API 资产 digest 一致。公开清单通过发布源码版本的 `mergeUpdaterManifests` 重新验证，结果完整结构相同，检查版本、componentPolicy、平台完整性、当前 tag URL 和签名资产存在。

首次只读下载遇到 TLS 连接中断，增加了有界网络重试，未关闭证书验证。第二次临时验收 harness 的 Node `-e` argv 使导入模块被误认为 CLI 入口，修正 harness 参数后第三次完整验收通过。未修改仓库发布验证器或放宽任何资产断言。

正常跳过的作业：非 workflow_dispatch 的 Promote Insider，以及未启用的 Homebrew cask 更新。SHA256SUMS 生成成功，不宣称本次更新了 Homebrew cask。

## 证据

本次证据根：`/Users/qs/.qin-code/scratch/worktree-hover-release-v01526-74x_fp6y`。

| 文件 | 内容 |
| --- | --- |
| `preflight.json`、`prepublish-tests.log` | 远程-only 边界、版本及 126 项测试 |
| `release-trigger.json` | 新 tag、固定提交与 push 触发的远程运行 |
| `run-status-*.json` | 按时间保存的 CI 元数据，包括失败、定向重跑及最终成功 |
| `windows-native-diagnosis/child-AUaf9k/diagnosis.md` | Windows 失败原始证据、依赖验证、恢复依据和未证实项 |
| `publication-run.json`、`publication-release.json`、`publication-tag.json` | 最终 run、公开 release 与 tag/source 身份 |
| `SHA256SUMS.txt`、`latest-stable.json`、`latest.json` | 实际匿名下载的公开元数据 |
| `validated-public-manifest.json` | 复用原发布验证器的校验结果 |
| `publication-acceptance.json` | 18:17:58 +08:00 `ok=true` 的最终机器验收记录 |
| `publication-verification-final.log` | 完整验收成功日志 |

公开资产包括 macOS arm64 DMG/app tar.gz、Windows x64 MSI/NSIS、Linux x64 `se-server`、适用签名、两个 manifests 和 SHA256SUMS，共 12 项。

| 检查对象 | SHA-256 / 说明 |
| --- | --- |
| 实际下载 SHA256SUMS | `1e0bb4de50d95284aa23aad8d200d09a0d548bcbe50e0cf0242036f95254a4de` |
| 实际下载的两个 manifests | `e79bad0f28e7624ff8cb1f4325c5478ee2705ee6df9db4966d5e4f3666716d0d` |
| macOS DMG，checksum/API 一致 | `0ce6bde32580013d6be06dbd08af2498b342b84d215546f2ec1d14ff1d1241d9` |
| Windows NSIS，checksum/API 一致 | `3e08c5254fc5d03c9c7c5127e547dbf6413c56a7c9d9eb2c185777d045bc8dc2` |
| Windows MSI，checksum/API 一致 | `13a358159b7fed4de76c164aea95705d4a730c9261b14a99d293e86a323411b3` |
| macOS app tar，checksum/API 一致 | `787c3bff9bc8dc4ff025455fe7b63598a266b9a3577a572ff3ab9ed5fd4ccde8` |
| Linux server，checksum/API 一致 | `d59e770db12e25d35de6fc08b98975d1365ea959006b64185a9f22df7b2541f3` |

## 残留风险

- 二进制安装包未下载到本机执行或安装，表中的二进制摘要是公开 checksum 与 API digest 对比，不是本机重算全部二进制字节。
- 未独立做 updater 签名的密码学验证或 Apple notarization 验证，不能由 CI 成功推断这些额外验收已执行。
- GitHub Release immutable 模式为 false。本次未移动 tag，不承诺未来引用无法被其他有权限的操作修改。
- 本次没有本地运行新版本的 UI smoke test，遵守不本地打包/安装的要求。发布流水线与公开元数据验收不等同安装后交互验收。
- 无当前发布阻塞。原有未跟踪文档、知识、锁文件和 pycache 保留，没有随发布提交或被清理。
