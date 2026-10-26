---
schema: qin-doc/1
kind: audit
id: quick-tabs-release-canary
title: 快捷多标签远程发布与Canary替换验收
date: 2026-10-26
status: draft
source: user:2026-10-26T16:57:57+08:00; commit:45f48a0b; github-run:40297303973; agent://verify-v01525-publication; agent://canary-archive-final-review; file:/Users/qs/.qin-code/scratch/quick-tabs-canary-release-IVm8yCR4/final-install-acceptance.json
updated: 2026-10-26
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-install-safety-acceptance"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-install"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-remote"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-verify"}],"goal_refs":[]}
---

## 结论

**远程发布和本地 Canary 替换均已完成。** 2026-10-26 19:25:38 +08:00 完成原子替换，19:25:48 独立安装 verifier 通过，19:26:09 父会话再次核验通过。

- 远程正式版本：[v0.15.25](https://github.com/qinsehm1128/termul-new/releases/tag/v0.15.25)，源码 `45f48a0b6b08483f875c46accb0455500985eb3a`。
- 唯一替换目标：`/Users/qs/Applications/Se Manager Canary.app`，版本由 `0.15.23-canary.1` 升至 `0.15.25-canary.1`。
- `/Applications/Se Manager.app` 保持 `0.15.24`，主 PID `8769`、核心 PID `8849`、`8855`、`8857` 的路径、PPID、启动时间及正式版关键文件指纹全部不变。系统目录旧 `/Applications/Se Manager Canary.app` 也未变。
- 未启动、关闭或重启任何应用，没有发送进程信号或向活 PTY 注入输入。新版 Canary 保持未启动。
- Canary 持久数据未变，旧 app 与完整持久数据归档已经校验并保留。所有原生应用仍进行完整物理文件树相等校验。

## 发现

### 远程发布与公开资产

- 发布运行 [40297303973](https://github.com/qinsehm1128/termul-new/actions/runs/40297303973) attempt 2 成功，Release 于 18:17:43 +08:00 公开。19:22:51 父会话刷新确认仍为同一成功运行、同一公开资产摘要。
- tag 对象 `d80b28992e4c8137cbbe4a8b10ad86dd2cae1502` 始终解析到原发布提交，没有覆盖 tag、强推或绕过 CI。
- attempt 1 的 Linux Checkout 失败且收尾卡住。正常取消完成后仅定向重跑 Linux 及依赖作业，原生链接、签名、身份及发布门禁全部保留。首次失败日志最终仍是 BlobNotFound，因此底层 Checkout/收尾故障原因未获证实，不虚构网络根因。
- 所有必需发布作业通过。Promote Insider 因非 workflow_dispatch 正常跳过。Homebrew cask 是 opt-in，未声称已更新 cask；校验和生成成功。
- 独立公开发布核验 29 项通过。匿名下载 macOS DMG、app tar、签名文本及 manifests/checksums。父会话重算全部 6 项下载文件 SHA-256，与公共资产摘要一致。
- `latest-stable.json` 与 `latest.json` 相同，6 个 updater 平台键齐全。manifest 无直接 commit 字段，来源通过 tag、Actions head、baked component identity 及源码再生成 IDs 验证。

### 同源码 Canary

- 从精确发布源码独立构建 `0.15.25-canary.1`，不是将正式版改名。arm64，身份 `com.se-manager.app.canary`，native/frontend Canary 编译标志及 Ghostty 真实原生链接已验证。
- ad-hoc `codesign --verify --deep --strict` 通过。updater endpoints 为空，不生成更新 artifacts，不读取或生成私钥。
- 2,107 个源码文件有来源记录。构建快照仅同步 Cargo.lock 根包版本 0.15.24→0.15.25，依赖版本/校验和不变，未改动仓库源码。
- 安装完整树 SHA-256 与已批准来源一致：`f65bce4a30324a5ffc2e5d1aaebffff765b3fe7ab1f794570d51dd9033c92025`。

### 真实备份差异与无损归档修复

18:32:58 第一次 apply 在备份阶段正确停止，尚未创建应用交换事务。根因为 32 项 WebKit 备份条目中的 14 项额外带有 `com.apple.provenance`，字节、类型、权限、链接和全部原有属性一致。当前进程环境无法可靠去除新增来源属性，因此停止重复同一复制尝试。

依据独立技术评估，改为持久数据无损归档表示，而不是忽略来源属性。归档逐项保留文件字节、类型、权限、路径、链接及全部原始扩展属性名称/值，精确区分不存在与空值。独立 decoder 验证结构和内容，manifest format 2 同时绑定容器摘要和解码树。旧 app、暂存新 app、已安装 app、保留原 app 仍是严格物理树相等。

- 最终独立审查 `canary-archive-final-review`：APPROVED，0 findings。
- 40 项 dummy tests 独立通过，附加 8 项内存 link graph/hierarchy 测试通过。
- 新增格式覆盖非法 schema、重复项、路径穿越、属性/负载篡改、硬链接/不安全链接、捕获期间变化、源独立读取、受限临时恢复和非精确恢复拒绝。
- 原有应用签名/架构、进程/socket、暂存/交换/保留/回滚保护未弱化。安装器、独立 verifier 和基线均未修改。
- 19:25:22 父会话校验最终获批文件哈希并运行新的真实只读 preflight 通过，才使用全新备份目录执行第二次 apply。

### 安装与验收

- 完整备份：`/Users/qs/.qin-code/scratch/quick-tabs-canary-release-IVm8yCR4/backup-v0.15.25-canary.1-archive-v2`。
- 保留原 app：`/Users/qs/Applications/.se-canary-install-iz0ubvml/original-canary.app`。
- receipt：`/Users/qs/.qin-code/scratch/quick-tabs-canary-release-IVm8yCR4/install-receipt-v0.15.25-canary.1-archive-v2.json`。
- backup manifest SHA-256：`a18159bf84351ae332884890dcbd0bfae945c9902ab7b06a573e0e8acfbbc943`。
- 原 app 完整树 SHA-256：`a55f16d297caaef6cfe09f9c63f3ca546768e1d853cf279bd2c75c42a2181f45`。
- 独立 `verify_installed.py` 返回 `ok=true`、`backup_verified=true`、`persistence_unchanged=true`、`stable_unchanged=true`、`protected_processes_unchanged=true`、`applications_launched=false`、`signals_sent=false`。
- 失败的第一份部分备份 `backup-v0.15.25-canary.1` 保留为诊断，不复用，不视为完整备份。

## 证据

证据根目录：`/Users/qs/.qin-code/scratch/quick-tabs-canary-release-IVm8yCR4`。

| 证据 | 内容 |
| --- | --- |
| `publication-verification/evidence.json` | 29 项独立公开发布核验、资产/hash、源码身份和签名限制 |
| `publication-gate.json` | 父会话对公开下载与脚本/基线的复核 |
| `parent-preapply-publication-freshness.json` | 19:22:51 发布、tag、资产及基线仍一致 |
| `build/build-result.json`、`build/source-provenance.json` | 独立 Canary 产物、精确来源及限定 lock 修正 |
| `build/parent-preapply-build-verification.json` | 父会话完整构建验证 PASS |
| `diagnosis-webkit-20261026/diagnosis-summary.md` | 实际备份属性差异及停止无效复制尝试的证据 |
| `approved-install-inputs.json` | 最终获批文件哈希、已安装状态及验收指向 |
| `preinstall-validation-archive-v2.json` | 第二次 apply 前真实只读预检通过 |
| `install-receipt-v0.15.25-canary.1-archive-v2.json` | 原子替换、保留原 app、备份及 manifest 摘要 |
| `installed-verification-archive-v2.json` | 独立安装后完整验证通过 |
| `final-install-acceptance.json` | 19:26:09 父会话新版本、主进程、正式版及旧系统 Canary 验收 |
| `legacy-canary-baseline.json` | 未触碰的系统目录旧 Canary 指纹与真实缺失项 |

关键 SHA-256：

- 公开 macOS DMG：`63959505da5563acabf5c9559935d97cfd78c82c7e21ae29a01cefde7f091b99`。
- 公开 macOS tar.gz：`0de37aa763059d2c99dadb658521151b5634e3d39c46bb4d870cf2869f33336f`。
- 已安装 Canary executable：`254559c4ebe7789dbe6914cb265245d91d4b5ebcb2c821833cc1625e08fe7291`。
- 获批 `canary_safety.py`：`a9efe5e1a4d1ddb2ca479d91781421052eeecd17f56bf3a037950c8d2b96791e`。
- 获批 `persistence_archive.py`：`18a6cb90124aabbd48b90650c10ef96c01c64f6c93d32cf327ce6c33a3baa0f6`。

受保护主 PID `8769` 启动于 08:52:40 +08:00，核心 PID `8849`、`8855`、`8857` 启动于 08:52:43 +08:00。19:26:09 核验时均与 17:02 原始基线一致。

## 残留风险

- 为遵守用户边界，未启动新版 Canary，未进行安装后的交互 smoke test。静态身份、签名、架构、全树、备份、数据和进程验收通过不等于运行体验验收。
- GitHub Release immutable 模式未启用。本次原 tag 对象和源码未变，不保证远端引用无法在未来修改。
- 远程 macOS 是 self-signed，本地 Canary 是 ad-hoc，都不能宣称 Developer ID/notarized。没有独立 Apple notarization 验证。
- Windows/Linux payload 通过公开清单、API digest/checksum 和成功 CI 验证，未在本机下载全部字节。updater 签名未独立密码学重验。
- 归档无损保存原始状态，但当前环境精确物理恢复不存在的 provenance 属性尚未证明可行。受限恢复函数遇到差异会停止并保留归档，不能直接恢复 live persistence。安装未改持久数据，应用回滚使用完整保留原 app，不依赖持久数据恢复。
- 这是协作用户环境事务保护，不是对抗恶意同账户并发修改的安全边界。树契约不覆盖 birthtime、inode、owner/group、POSIX ACL；未对这些额外属性作保真承诺。
- 无当前任务阻塞。保留所有备份和恢复材料，不自动清理。
