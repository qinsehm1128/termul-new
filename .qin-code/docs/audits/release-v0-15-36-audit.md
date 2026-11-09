---
schema: qin-doc/1
kind: audit
id: release-v0-15-36
title: Se Manager v0.15.36正式发布验收
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: GitHub workflows/release, Git remote refs and actual downloaded asset verification
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"37"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"38"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"39"}],"goal_refs":[]}
---

## 结论

Se Manager **v0.15.36** 于 **2026-11-07 23:23:57 +08:00** 正式发布。Release非draft、非prerelease，Latest与公开更新清单均指向0.15.36。三平台构建、安全扫描及实际下载资产核验通过。

发布地址：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.36

## 发现

- 本版本包含`cc1e49774b26d71c6ed747acc8f36551885b7a5d`：Se内置MCP与外部上游聚合器的真实来源隔离。
- 内置连接key为`se-mcp`，聚合器为`se-mcp-aggregator`。独立settings/端口/token及调用边界，保留记忆、SSH授权/黑名单和se-memory退役。旧无source连接收窄到internal，外部上游需显式接入aggregator。
- 发布提交：`f698b642b9bc34702afd1190ee42defd1cfddf7f`。
- Annotated tag object：`8b5beceaba95337722c8c8a7db7bc78d1b624ccf`，peeled commit与发布提交一致。
- main/tag原子推送，未覆盖旧tag或force push。未自动安装、部署或修改真实连接配置。

## 证据

### 发布前

任务`650441jjsk`退出0：三版本源与stamped Cargo.lock一致为0.15.36，Cargo locked metadata通过；bridge协议24测试、12文件364项前端及发布脚本测试、完整typecheck、脚本语法和catalog检查通过。catalog为327 registered commands，unclassified:0。功能完整Goal与独立分析证据见`mcp-source-isolation-verification-audit.md`。

### 远端流水线

- Release `42384326858` success：https://github.com/qinsehm1128/termul-new/actions/runs/42384326858
- Secret Scan `42384326633` success。
- CodeQL `42384326726` success。
- Windows x64、macOS aarch64、Linux x64 se-server构建，以及Validate/upload/publish、SHA256SUMS生成均success。
- Insider promotion、未配置Homebrew更新/AUR发布按条件跳过。
- Git远端tag对象及peeled commit匹配，GitHub latest API返回v0.15.36。

### 实际下载校验

下载全部12个发布assets，逐项文件大小及SHA256与GitHub资产元数据一致。SHA256SUMS.txt的6项通过。6个updater platform URL均指向本tag实际资产，签名字符串与对应.sig资产一致。

以下四份清单逐字节一致，version与componentPolicy.targetVersion均为0.15.36：tag资产latest.json、latest-stable.json，以及公开releases/latest/download下对应两份清单。

复验命令：`python3 .qin-code/scratch/release-v0.15.36-verification/verify.py`。退出0，assetCount=12、checksumCount=6、platformCount=6、publicManifestsByteIdentical=true。完整摘要与原下载文件保留在同目录的verification.json及assets。

## 残留风险

- 校验覆盖摘要和签名文件一致性，没有另行进行密码学签名验签，也没有执行下载的二进制或安装包。
- 流水线仍有既有Node action弃用及runner镜像迁移提示，不构成本次失败。
- 发布完成不代表本机已安装新版。本轮未重启用户应用、操作PTY或真实SSH，也未自动切换客户端连接。
- 用户.gitignore和scripts/release/__pycache__保持未提交，未纳入tag。
