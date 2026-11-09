---
schema: qin-doc/1
kind: audit
id: release-v0-15-35
title: Se Manager v0.15.35正式发布验收
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: Git remote refs, GitHub Release/CodeQL/Secret Scan and downloaded asset verification
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"30"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"31"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"32"}],"goal_refs":[]}
---

## 结论

Se Manager **v0.15.35** 于 **2026-11-07 19:22:41 +08:00** 正式发布，Latest已指向本版本。Windows x64、macOS aarch64及Linux x64 se-server构建成功，实际下载资产与公开更新清单核验通过。

发布地址：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.35

## 发现

- 本版本包含 `743b91fd` 完整se-mcp配置展示及 `3b599c2e` 独立se-memory退役。最终界面只保留统一se-mcp导出，记忆功能与数据仍保留。
- 发布提交：`f83480f7dc504a5a0f06dd6479e8cba1ee6de6db`。
- Annotated tag object：`4a5a64b2a5c7bd6c66e1a3872c6ac3938432ee5e`，peeled commit等于发布提交。
- main与tag原子推送，未覆盖旧tag或force push。用户.gitignore与scripts/release/__pycache__未提交。
- Release非draft、非prerelease。未自动安装或部署新版。

## 证据

### 发布前

任务`229782zjio`退出0：三版本源与stamped Cargo.lock统一0.15.35，Cargo locked metadata通过，9个Vitest文件240测试通过，完整typecheck、发布脚本语法检查和catalog检查通过。catalog为327 registered commands、unclassified:0。

功能完整验收见 `mcp-entry-display-verification-audit.md` 与 `legacy-se-memory-retirement-audit.md`。后者记录最终196项前端测试及Rust记忆/ACP/统一provider/退役回归、desktop与standalone检查，并明确保留既有1项ignored测试。

### 远端

- Release `42366354236` success：https://github.com/qinsehm1128/termul-new/actions/runs/42366354236
- Secret Scan `42366354071` success。
- CodeQL `42366354082` success。
- 三平台build及Validate/upload/publish、SHA256SUMS生成全部success。Insider promotion、未配置Homebrew/AUR发布按条件跳过。
- Git远端tag对象和peeled commit与本地一致，GitHub latest API返回v0.15.35。

### 下载产物

主会话下载并验证全部12个assets：文件大小及SHA256与GitHub资产元数据全部一致。SHA256SUMS.txt六项通过。六个updater platform URL均指向本tag真实资产，签名字符串与对应.sig资产一致。

latest.json、latest-stable.json及公开releases/latest/download下对应两份清单逐字节一致，version和componentPolicy.targetVersion均为0.15.35。

验证脚本与完整摘要：`.qin-code/scratch/release-v0.15.35-verification/verify.py`、`verification.json`。执行退出0，assetCount=12、checksumCount=6、platformCount=6，publicManifestsByteIdentical=true。

## 残留风险

- 本次产物校验验证摘要和签名文件一致性，没有另行做密码学签名验签，没有执行下载的程序或安装包。
- 流水线有既有Node action运行时弃用及runner镜像迁移提示，未构成失败，未为本次发布修改无关CI。
- 发布完成不等于本机已升级。未自动安装、重启用户应用、操作PTY或连接真实SSH。
