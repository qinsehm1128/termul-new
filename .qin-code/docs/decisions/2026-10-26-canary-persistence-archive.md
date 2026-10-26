---
schema: qin-doc/1
kind: decision
id: canary-persistence-archive
title: Canary持久数据无损归档备份决策
date: 2026-10-26
status: draft
source: user:2026-10-26T16:57:57+08:00; agent://canary-webkit-backup-debug; agent://canary-provenance-backup-decision
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-install-safety-acceptance"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"canary-release-install"}],"goal_refs":[]}
---

## 决定

为当前同源码 Canary 安装采用**持久数据无损归档、应用保持物理完整副本**的备份表示。此决定只授权受限实现和测试，不等于授权立刻重试安装。安装仍必须等待实现、独立复审、输入哈希重新冻结、真实只读预检以及父会话明确执行。

- 所有 Canary 持久数据根改用内容归档，逐项保存原始字节、类型、权限、相对路径、符号链接目标，以及全部扩展属性的名称和原始二进制值。完整属性集合必须区分不存在与存在但空值，不忽略 `com.apple.provenance`。
- 归档封装文件自身被 macOS 附加的属性不代表其内部源元数据。归档内部数据必须精确解码、重建原树清单，与稳定的源清单相等，容器字节摘要也必须被 manifest 绑定。
- 原 Canary app 的物理备份、暂存新 app、安装后 app、保留的原 app 均继续完整文件树相等校验，不做任何元数据标准化。
- 当前失败的部分备份不改动、不复用。后续尝试使用新的独占备份目录和 receipt。
- 不变更 live persistence，不启动或停止任何应用，不向主进程、核心进程或 PTY 发送信号或输入，不提权，不修改系统安全设置。

## 原因

第一次真实安装于 2026-10-26 18:32:58 +08:00 在备份阶段正确停止，未进行 app exchange。已确认 WebKit 源与部分备份各有 32 项，其中 14 项仅多出 `com.apple.provenance`。数据内容、类型、权限、符号链接和所有原有属性均一致，源清单重复读取稳定。

隔离实验表明当前进程及子进程生成文件时会带上来源属性，删除或修改该属性即使返回成功，读取结果仍保留。五轮假设检验没有证明可安全生成完全相同的物理副本。因此停止重复删除/复制，不将未证实的系统 API 或权限变更当作解决方案。

独立 analyst 比较了无损归档、物理副本附带受限属性差异，以及独立终端上下文三个方案。无损归档保存完整原始信息，不把来源属性判定为可信内核产物，也不引入“忽略所有 provenance”的通用例外，因此选择归档表示。用户授权的是安全替换 Canary，而不是限定备份容器格式。

## 后果

### 必须实现并验证

1. 使用严格、版本化格式，完整覆盖原有持久根及显式 IPC 排除记录。只有原有精确 core-host 链接白名单可例外允许绝对链接。
2. 只读/no-follow/identity 检查读取源文件，前后校验 inode、link count、size、mtime/ctime、属性及完整源清单。源硬链接按各路径独立保存内容，归档输出不能是硬链接或符号链接。
3. 归档独占创建、限制权限、flush/fsync，manifest 精确绑定来源、原基线、容器 SHA-256、解码后的树和覆盖范围。
4. 独立读取逻辑必须拒绝重复/歧义路径、路径穿越、未定义字段、畸形元数据、未知节点、非法链接、缺失/额外负载、容器或元数据篡改。
5. 现有 app 签名/架构、进程/socket、原子交换、保留旧 app、自动 app-only 回滚测试保持有效，不删除或弱化测试。新增缺失/空/二进制属性、所有节点类型、capture mutation、archive tamper 及恢复失败测试。
6. 提供不依赖 live source 的恢复读取能力。任何物理恢复仅可写入新的独占临时树，必须完整读回校验，差异时停止且保留归档。不得直接恢复或覆盖 live persistence。

### 明确限制

归档可以无损保留“应当存在的字节与元数据”，但不证明当前受跟踪进程能立刻物化一个没有来源属性的树。真实环境中精确物理恢复尚未证明可行，必须如实披露。安装本身不改持久数据，原应用回滚继续使用完整物理 app 副本，不依赖持久数据恢复。

不将本决定视为提升安全权限或绕过已失败校验的许可。若实现或复审不能满足上述条件，保持停止。源码、基线、正式版及系统目录旧 Canary 不得为迁就新格式而修改。
