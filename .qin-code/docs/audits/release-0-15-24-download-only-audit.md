---
schema: qin-doc/1
kind: audit
id: release-0-15-24-download-only
title: Se Manager v0.15.24正式发布与只下载验收
date: 2026-10-24
status: draft
source: user:2026-10-24T11:42:55+08:00; commit:eff547348a3d8cbd276436d5b92710246ddbf090; run:40060215052; file:/Users/qs/Downloads/se-manager-v0.15.24/release-verification.json
lang: zh-CN
---

## 结论

`v0.15.24`已完成正式发布，并已下载适配本机macOS Apple Silicon的安装包。2026-10-24 12:22:22 +08:00完成发布与下载验收，随后对最终路径再次复算SHA256通过。

- Release：https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.24
- 发布源码：`eff547348a3d8cbd276436d5b92710246ddbf090`
- 工作流：https://github.com/qinsehm1128/termul-new/actions/runs/40060215052
- 本地安装包：`/Users/qs/Downloads/se-manager-v0.15.24/Se.Manager_0.15.24_aarch64.dmg`
- 大小：50,342,637字节，约50.3MB。
- **未挂载、未安装、未启动新包，也未关闭或替换现有Se/Canary。**

## 发现

本次包含新建项目终端/屏幕观察引擎及gitignore选项、鼠标模式恢复修复、活动文件与文件树联动。正式tag是新创建的`v0.15.24`，没有覆盖旧tag或绕过门禁。

必需Release任务均成功：组件身份、changelog、Linux x64 se-server、macOS arm64、Windows x64、发布资产和SHA256SUMS。Insider晋升与未配置的Homebrew cask更新按条件跳过，不属于失败。

签名沿用仓库现有MACOS_CODESIGN通道，未配置Apple Developer ID公证通道。不能将该下载包称为Apple公证通过的包。本次没有读取或输出密钥内容，也没有变更签名配置。

## 证据

### 发布与下载验证

- 非draft、非prerelease正式Release共有12个资产，包含macOS DMG及updater tar/signature、Windows MSI/NSIS及签名、Linux server及签名、latest/stable清单、SHA256SUMS。
- `latest.json`与`latest-stable.json`字节相同，公开`releases/latest/download/`入口均返回0.15.24。
- 更新清单6个平台键齐全，资源URL均属于本仓库`v0.15.24`，所有对应签名字段非空。
- renderer/guiNative/acpCore/terminalCore的canonical build ID与本次提交生成的本地期望身份全部一致。
- GitHub tag object解析到`eff547348a3d8cbd276436d5b92710246ddbf090`。发布成功后再次查询仍一致。
- 下载目录内5个文件：DMG、SHA256SUMS.txt、latest.json、latest-stable.json、release-verification.json。
- DMG SHA256同时匹配GitHub资产digest、SHA256SUMS以及最终路径复算：

```text
6d1848c1a590b8eb7f7e3f19cefc90ce4c3f3b2da746e92dfd6d950514ba0592
```

- `hdiutil verify`报告DMG所有分区校验通过，最终checksum VALID。未使用attach、open或安装命令。
- 重新枚举`/Applications`和`~/Applications`的正式/Canary bundle identity集合，与发布前3个程序的路径、版本、主程序SHA256完全一致。

### 预发布检查

- 19个文件/537项功能回归通过；node/web/test完整类型检查通过。
- 8个文件/87项发布契约测试通过。
- se-pty 228项通过、2个原有生产native-host集成测试因无环境变量忽略；正式CI仍执行原有Ghostty native门禁。
- 工具链Python测试12项通过，passive-link审计测试18项通过。

### 下载中断与安全处理

- 首次本地watcher命令在120秒超时，远端CI并未失败；改用持久watcher后观察到整条工作流成功退出。
- 首次下载遇到qin-code服务重载，下载任务中断在28,334,208字节，未把部分文件交付到Downloads。部分文件保存在scratch并重新下载完整DMG；已完整的小清单复用前重新核对大小与digest。
- 独立脚本审查发现目标rename的no-overwrite竞态和应用集合检查不完整。已改为目标目录独占mkdir与文件`xb`写入，复核全部目标Se应用路径集合。隔离测试验证拒绝已有空目录、非空目录和文件，成功复制后再次核对hash。
- 完整证据位于`/Users/qs/.qin-code/scratch/se-release-0.15.24-KbjtpqkS/`，下载目录同时包含机器可读验收记录。

## 残留风险

- 按用户要求只下载，不验证安装后GUI或新core运行状态，不宣称本机现有应用已经升级。
- macOS包不是本次经过Apple公证的产物，安装时仍可能遇到系统信任提示。
- 原有非失败测试告警及native-host条件忽略保留在日志中，没有跳过或削弱测试以获得通过。
