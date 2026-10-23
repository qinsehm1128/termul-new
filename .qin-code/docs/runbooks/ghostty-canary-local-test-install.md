---
schema: qin-doc/1
kind: runbook
id: ghostty-canary-local-test-install
title: Ghostty Canary 本地测试安装与回滚
date: 2026-10-23
status: draft
source: user:2026-10-23T17:59:39+08:00; ask:2026-10-23T18:11:07+08:00; agent://build-ghostty-canary; agent://review-canary-install-safety; goal:passed:2026-10-23T18:40:11+08:00
lang: zh-CN
---

## 适用

用户于2026-10-23 17:59:39 +08:00请求安装一个与正式Se Manager不同名的测试包，随后明确选择升级既有 **Se Manager Canary**，而非新增第三身份。

已安装：`~/Applications/Se Manager Canary.app`，版本 **0.15.23-canary.1**，bundle ID `com.se-manager.app.canary`，Apple Silicon arm64。本地ad-hoc签名，非商店/Developer ID公证发行包。未自动启动。

现有正式版 `/Applications/Se Manager.app` 仍为0.15.21，未覆盖、未关闭或重启，既有终端进程未收到信号。

## 步骤

### 已完成的构建与安装

1. 构建源码 `9419976d5a9f2f0493c18784ce30dddd5139cef2`，相较正式v0.15.23源码`74101363`只有发布文档变化，无功能差异。
2. 在独立scratch target编译，设置`SE_CANARY_BUILD=1`、`VITE_SE_CANARY=1`、`VITE_APP_VERSION_OVERRIDE=0.15.23-canary.1`，合并prod和canary配置。prod的`ghostty-vt`原生功能保留。
3. 本地覆盖`plugins.updater.endpoints=[]`及`bundle.createUpdaterArtifacts=false`，避免本地测试包自行切回正式或其他渠道。
4. 使用经过manifest/哈希验证的Ghostty预编译静态库。最终可执行文件包含Ghostty VT和observer符号，构建静态库的12个Mach-O对象通过passive-link审计。
5. 确认旧Canary没有活动应用进程、3292监听者或core socket所有者。备份旧程序、Application Support数据和WebKit存储，排除2个临时IPC socket。
6. 备份修订为全新暂存目录、逐树内容哈希校验、原子发布，不合并或覆盖旧备份。安装前后验证清单范围、路径与全部持久数据树，并确认正式版plist/主程序SHA256不变。独立安全复核approved。
7. 18:39:54 +08:00仅替换用户目录Canary，保留旧程序原件和完整校验备份。18:40:11 +08:00 Goal机械验收passed。

### 如何测试

打开以下应用，而不是Dock里的正式版Se Manager：

```sh
open "$HOME/Applications/Se Manager Canary.app"
```

在Canary项目设置中找到 **终端屏幕观察引擎**，选择 **Ghostty VT（实验性）**，保存后新建终端。

这改变的是宿主屏幕观察，PTY与xterm渲染保持不变。只对新建或重新生成的终端生效，接管已有会话不会改变其引擎。不要将它与“Ghostty快捷键方案”混淆。

沿用的是旧Canary数据，不是正式版的配置和数据库。Canary的主要隔离边界为独立bundle ID、Application Support/WebKit、core sockets、`.se-manager-canary`、Canary keychain及网关3292。测试版可能需要单独授予macOS隐私权限。现有profile中SSH known-hosts文件等少量非密钥全局约定仍共享，不将Canary称为完整OS沙箱。

## 失败时

本次证据与回滚目录：

```text
~/.qin-code/scratch/se-ghostty-test-install-1Feqm7A6/
  build-result.json
  preinstall-validation.json
  installation.json
  baseline.json
  backup-verified/manifest.json
  backup-verified/app/
  backup-verified/app-data/
  backup-verified/webkit/
  original-app-before-upgrade.app/
  verify_installed.py
```

- 若无法启动，先记录系统提示和Canary日志，不禁用Gatekeeper、不修改正式版。
- 如需回滚，先关闭Canary及其子进程，保留正式版继续运行。旧应用原件为`original-app-before-upgrade.app`，原数据快照在`backup-verified`。
- 新版启动后可能迁移Canary数据。回滚不应只降级程序而继续读已迁移数据库，应在用户确认后成套恢复对应旧Canary快照。
- 安装脚本发现Canary活动、备份不完整、路径身份不符或正式版指纹变化时会拒绝安装。替换后验证失败会恢复原程序。没有删除任何已有程序备份或用户数据。

## 验证

- Canary profile测试：3项通过。
- Tauri release构建：成功，正确身份与0.15.23-canary.1版本。
- `codesign --verify --deep --strict`：通过，arm64架构验证通过。
- 新程序SHA256：`895cb6b3ab6d2ec52004af710734e3baf57a7c7ba4b59ffba6e0597a68246d78`。
- 真实Ghostty符号：父会话检出7项，静态库12个Mach-O对象审计通过。
- 三个备份树完整验证通过，重复写入canonical备份被拒绝。
- 正式版plist与主程序SHA256仍与安装前相同，Canary持久数据在安装前后与快照一致。
- 已执行安装验收脚本并由Goal标记passed。**未启动应用，未声称已完成GUI交互或功能体验测试。**
