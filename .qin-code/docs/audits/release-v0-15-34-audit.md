---
schema: qin-doc/1
kind: audit
id: release-v0-15-34
title: Se Manager v0.15.34正式发布验收
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: Release workflow, Git remote refs, main-session downloaded asset verification
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"21"}],"goal_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"goal-d0abd11f-f584-4a26-a910-d6f3b1231f9c"}]}
---

## 结论

Se Manager **v0.15.34** 已于 **2026-11-07 17:26:10 +08:00** 正式发布。先完成 MCP-only SSH 命令黑名单实现、独立源码复核、完整机械验收和功能提交，再升版及原子推送新tag。Release流水线成功，发布产物及公开自动更新清单校验通过。

发布地址：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.34

## 发现

- 功能提交：`10b26f578abab6d0229ba35e37248239c1747f4d`。
- 发布提交：`53a638d86d856fa37168cfb4744e9d92bfea5001`。
- Annotated tag object：`a335debe60659f7688b78fe816abb6ce4057cb1a`，peeled commit与发布提交一致。
- Release非draft、非prerelease。Windows x64、macOS aarch64、Linux x64 se-server均成功构建。
- 黑名单只作用于MCP execute/job_start及其内部managed-job入口。普通本地终端、SSH交互终端和PTY未加入策略拦截。
- 用户.gitignore与scripts/release/__pycache__未纳入提交。未安装新版本、未连接真实SSH、未触碰用户PTY。

## 证据

### 本地验收

详见 `ssh-command-deny-verification-audit.md`。完整Goal机械验收通过，10项HTTP预览测试、218项前端测试、类型检查、双前端构建、standalone构建和插件目录检查通过。版本对齐0.15.34，Cargo locked metadata有效，44项发布脚本测试通过。插件目录329 registered commands，unclassified:0。

### 远端流水线

- Release：`42358275312`，success。https://github.com/qinsehm1128/termul-new/actions/runs/42358275312
- Secret Scan：`42358275319`，success。
- CodeQL：`42358275370`，success。
- Insider promotion、未配置的Homebrew更新和AUR发布按现有条件跳过，不属于平台构建失败。

### 下载与更新清单

主会话实际下载全部12个assets，每个文件大小与GitHub元数据相符，SHA256均与GitHub asset digest一致。SHA256SUMS.txt内6项全部通过。6个updater platform URL均定位到本tag真实资产，签名字符串与相应.sig文件一致。

以下四份清单逐字节一致，version与componentPolicy.targetVersion均为0.15.34：

- tag资产 latest.json
- tag资产 latest-stable.json
- 公开 releases/latest/download/latest.json
- 公开 releases/latest/download/latest-stable.json

可复验脚本与结果存于 `.qin-code/scratch/release-v0.15.34-verification/verify.py`、`verification.json`，原下载文件与release.json位于同目录。执行命令：`python3 .qin-code/scratch/release-v0.15.34-verification/verify.py`，退出0，assetCount=12、checksumCount=6、platformCount=6。

## 残留风险

- 产物验证原委派执行器因provider认证不可用而失败，未产生下载目录。主会话接手完成实际下载校验，不把此部分称为独立执行验证。
- 本次核验包括摘要与签名文件一致性，没有另行做独立密码学签名验签，也没有执行下载的二进制或安装包。
- 黑名单是原始命令文本防护，不是shell沙箱。完整实施边界和最终分发窗口限制见功能验收审计。
- 新版已发布，但未自动安装或部署到用户机器及服务器。
