---
schema: qin-doc/1
kind: audit
id: release-v0-15-37
title: Se Manager v0.15.37正式发布验收
date: 2026-11-08
status: draft
source: Local tests/builds, GitHub workflows/release, remote refs and downloaded assets
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"42"}],"goal_refs":[]}
---

## 结论

Se Manager **v0.15.37** 已于 **2026-11-08 00:35:26 +08:00** 正式发布，Latest和公开自动更新清单均为0.15.37。更新、版本对比、运行状态与重启现位于应用偏好设置同一组的相邻分区，原安全动作保持不变。

发布地址：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.37

## 发现

- 布局提交：`e0cfd0041b002d48763d0f74b39740d1d786f5d7`。
- 发布提交：`e809ff331f8732e5426d63033e0809f691fc57cf`。
- Annotated tag object：`77aeb48f5077358d9e2d4be94cd4109e12a0218d`，peeled commit与发布提交一致。
- Release非draft、非prerelease。main/tag原子推送，无force push或覆盖旧tag。
- 本次仅布局重排及本地化，不改变更新、强制安装、确认弹窗或Core重启逻辑，详见update-runtime-cohesion-audit.md。

## 证据

### 本地验收

主会话布局验收7文件100测试、完整typecheck、Tauri/Web双前端构建通过，独立静态审查无阻塞。升版后版本/锁文件一致性及Cargo locked metadata通过，包含发布脚本的10文件144测试通过，catalog327 registered commands、unclassified:0。

### 远端流水线

- Release `42390125148` success：https://github.com/qinsehm1128/termul-new/actions/runs/42390125148
- CodeQL `42390124910` success。
- Secret Scan `42390124920` success。
- Windows x64、macOS aarch64、Linux x64 se-server及发布/校验和生成全部success。
- Insider promotion、未配置Homebrew/AUR更新按既有条件跳过。

### 实际产物

下载12个assets并逐项核对大小与GitHub SHA256 digest，全部通过。SHA256SUMS.txt的6项通过。6个平台URL指向本tag真实资产，签名字符串与对应.sig文件一致。

tag资产latest.json/latest-stable.json与公开releases/latest/download下对应两份清单逐字节一致，version及componentPolicy.targetVersion均为0.15.37。GitHub latest API和远端tag对象/peeled commit亦已核对。

证据目录：`.qin-code/scratch/release-v0.15.37-verification/`。复验命令`python3 .qin-code/scratch/release-v0.15.37-verification/verify.py`退出0，assetCount=12、checksumCount=6、platformCount=6、publicManifestsByteIdentical=true。

## 残留风险

- 未进行真实原生窗口像素测试、安装新版或触发更新/重启；UI结构与交互通过jsdom和独立源码审查验证。
- 资产校验为摘要和签名文件一致性，未独立做密码学签名验签，未执行下载的程序。
- AppPreferences既有33条a11y warning仍保留，正常Biome检查退出0，不声称无警告。CI既有Node弃用/runner迁移提示不构成本次失败。
- 用户.gitignore和scripts/release/__pycache__未提交。现有应用和PTY未被停止，未自动安装。
