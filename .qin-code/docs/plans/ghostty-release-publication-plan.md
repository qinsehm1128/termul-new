---
schema: qin-doc/1
kind: plan
id: ghostty-release-publication
title: Ghostty VT 正式发布进度与失败修复计划
date: 2026-10-23
status: draft
source: user:2026-10-23T16:49:36+08:00; commit:74101363; github-run:39928758964; release:v0.15.23; goal:passed:2026-10-23T17:35:23+08:00
updated: 2026-10-23
lang: zh-CN
---

## 目标

**已完成：Ghostty VT 宿主屏幕观察引擎修复版 `v0.15.23` 正式上线。**

用户请求发布上线及 tag，禁止本机安装。用户 2026-10-23 16:49:36 +08:00 补充“Windows 可以先不检测”，因此本次跳过额外手动 Windows preflight，直接走正式 Release 流水线。正式构建、签名和被动链接审计未弱化。

公开 Release：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.23

## 非目标

- 未在本机安装或启动新包，未运行应用安装/更新脚本。
- 未擅自部署或重启 VPS。
- 未强推、覆盖或删除已有 tag，原 `v0.15.22` 保持不动。
- 未增加 Intel Mac/Linux 桌面目标。支持 Windows x64、macOS arm64、Linux standalone `se-server`。

## 任务

### 发布完成证据

- 发布源码：`74101363d95dfd61126db6b6e3d820615b1f2e55`。
- annotated tag：`v0.15.23`，tag 对象 `5306ce8441d34fc6dc97d77195f382f03bd4ba01`，peel 与发布源码相同。
- 17:00:49 +08:00 原子快进推送 main 和新 tag 成功。
- 正式 run：https://github.com/qinsehm1128/termul-new/actions/runs/39928758964 。最终 `completed/success`。
- Windows x64、macOS arm64、Linux server 三个构建 job 均 success。`Validate, upload, and publish release`、checksum metadata resolve 与 SHA256SUMS job 均 success。可选 Homebrew cask job 按工作流条件 skipped，不将其称为已更新。
- 正式发布于 **2026-10-23 17:33:35 +08:00**，非 draft、非 prerelease，共12个上传资产。
- **17:35:23 +08:00 Goal 机械验收通过**，公开 latest/stable 清单均指向0.15.23。
- 旧 loop `l396`、`l397` 已取消。最终容错只读 watcher `609258lhph` 已正常退出0，无须再持续监听。

### 故障与修复记录

1. 首个 `v0.15.22` / `f66fb55fd16d5726b3af47031e71a48ac9b636d6` 的 run `39917645711` 仅 Windows passive 审计失败，macOS/Linux server 成功。原 tag 不移动，失败草稿不手动发布。
2. Windows native VT 本身通过，旧审计器不认识 `ntdll.lib` 内嵌 ar（magic `213c6172`）。`50495873` 新增诊断预检，run `39919761413` 取得真实静态库，不是安装包。
3. `951d3628` 增加有界嵌套 ar 和严格必要的 COFF import 支持，未知或畸形格式继续拒绝。父复验当时的 Python3.11/3.12 各16测试、Windows3627对象、macOS12对象通过。
4. 独立 review 发现扩展 COFF relocation 范围少算marker。`74101363` 修正完整范围、marker保留字段及与headers/section/symbol/string数据的重叠拒绝。18项离线回归由worker和父分别通过。最终Windows正式构建的真实原生审计也成功。
5. `ac1e2308` 修复五个passive self-test/audit步骤的PowerShell失败掩盖问题，显式bash + `set -euo pipefail`。可选手动预检完整SHA绑定与诊断必需性经独立复核approved，但依用户请求本次未再触发它。
6. `6d8354a4` 对齐0.15.23三个源码版本和精确根锁条目。版本、locked Cargo metadata、三个TS工程、240项发布/parity测试和Biome通过。parity保留既有推荐测试缺失警告，未跳过或弱化测试。
7. `gh run watch` 多次网络EOF以及一次本地进程timeout均未误判为远端失败。最终改为有界、有限网络重试的只读API watcher。它不改变发布状态、不下载或执行资产。

诊断静态库保存在会话scratch，既有knowledge/reference lock与生成的`__pycache__`未纳入本次提交。

## 验收

已成功运行：

```text
python3.12 /Users/qs/.qin-code/scratch/se-release-verify-G6Y36RPQ/verify_release.py --version 0.15.23 --sha 74101363d95dfd61126db6b6e3d820615b1f2e55 --run 39928758964
```

检查全部通过：

1. 正式workflow completed/success，commit与tag精确一致。
2. Release非draft/非prerelease，GitHub latest指向新版本。
3. 桌面包、server、签名、SHA256SUMS、latest.json、latest-stable.json均uploaded且非空，共12资产。
4. 公共latest/stable版本正确，各平台具有非空签名，下载URL指向该tag已上传资产。
5. 未本机安装或启动新包，未擅自操作VPS。

公开更新入口：https://github.com/qinsehm1128/termul-new/releases/latest/download/latest.json
