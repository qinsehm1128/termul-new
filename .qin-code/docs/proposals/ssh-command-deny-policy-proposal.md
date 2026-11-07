---
schema: qin-doc/1
kind: proposal
id: ssh-command-deny-policy
title: SSH命令黑名单与宿主执行门禁方案
date: 2026-11-07
status: draft
source: 用户需求req-8f01105f55c188c39fdf0e9609e35343；agent://ssh-command-deny-design；classfang/ssh-mcp-server@ff97574651f6e1efe938797ba29f526c499c36a3
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"16"}],"goal_refs":[]}
---

## 问题

用户希望参考 classfang/ssh-mcp-server，为 Se 已有内置 SSH 能力设计可配置命令黑名单。本文件是待实施方案，不表示黑名单已经落地。界面简化与「新授权默认全部13操作」已独立实现，不因本方案改变。

### 已核实的参考实现

2026-11-07 重新核实参考项目 main 为 `ff97574651f6e1efe938797ba29f526c499c36a3`，版本 1.11.0。

- 每连接配置 `commandBlacklist: string[]`，编译成 JavaScript RegExp，对原始 command 做 `.test()`，任意命中即拒绝。默认区分大小写，不隐式加锚，不解析 shell。
- `execute` 和 `jobStart` 都检查，拒绝检查早于请求触发的 SSH 连接。上游还支持白名单，但 blacklist-only 模式不额外禁止所有 shell 控制字符。
- 不能照搬 CLI 逗号/竖线分隔字符串，可能与正则量词和 alternation 冲突。上游此处未给规则总量与编译资源上限。

固定来源：
- [配置类型](https://github.com/classfang/ssh-mcp-server/blob/ff97574651f6e1efe938797ba29f526c499c36a3/src/models/types.ts#L17-L18)
- [编译与匹配逻辑](https://github.com/classfang/ssh-mcp-server/blob/ff97574651f6e1efe938797ba29f526c499c36a3/src/services/ssh-connection-manager.ts#L1981-L2049)
- [execute 与 jobStart 前置验证](https://github.com/classfang/ssh-mcp-server/blob/ff97574651f6e1efe938797ba29f526c499c36a3/src/services/ssh-connection-manager.ts#L2101-L2211)
- [被拒绝任务不运行远端命令的测试](https://github.com/classfang/ssh-mcp-server/blob/ff97574651f6e1efe938797ba29f526c499c36a3/test/ssh-connection-manager.test.js#L2721-L2732)

## 方案

### 1. 两层规则，只能叠加不能绕过

新增宿主全局规则及每个 SSH profile 的追加规则。有效拒绝为 **全局 OR 当前连接，任意命中即拒绝**。连接不能覆盖全局拒绝，不提供允许例外。规则只限制已授予的 execute/job_start，不授予权限，不改变身份 pin、根目录或 POSIX 许可。

建议 HostAccessPolicy 升为 schemaVersion=2：

```ts
interface CommandDenyRule {
  id: string
  kind: 'exact' | 'contains' | 'regex'
  pattern: string
}
// HostAccessPolicy.ssh.commandDeny: CommandDenyRule[]
// HostAccessPolicy.ssh.profiles[id].commandDeny: CommandDenyRule[]
```

- `exact`：整个原始字符串相等。
- `contains`：字面子串包含，不把特殊字符当成正则。
- `regex`：高级模式，使用 Rust regex search，默认大小写敏感，锚点和多行行为由规则明确指定。
- 不自动 trim/压缩空白/去引号/展开变量或脚本，不做 Unicode 或大小写归一化。匹配的必须与实际执行的用户 command 相同。
- 空数组表示该层没有额外拒绝。默认不自动启用一套自称完整的危险命令规则，可提供带说明的可选模板。

初步资源限制建议：每层32条，总策略128条，pattern≤512 UTF-8 bytes，command保持现有32KiB上限。RegexBuilder 设置编译体积、DFA与嵌套限额；具体数值在实现时基准校准。Rust regex 不支持回溯引用/lookaround，不能假装与 JS RegExp 完全兼容。保存/启动加载时整份校验，不忽略非法规则，按revision持有不可变编译快照，禁止逐请求编译。官方依据：[Rust regex](https://docs.rs/regex/latest/regex/)。

### 2. 统一宿主门禁，不依赖前端

在共享 Rust 域新增纯 `ssh/command_policy.rs` evaluator，扩展当前 CallContext 授权链，直接调用 automation/JobManager 也不能绕过。

顺序：

1. 纯参数校验与取消检查，提前解析 command/timeout/cwd 等字段。
2. 从同一权威策略快照校验宿主启用、固定 caller、profile 操作授权、身份 pin、job_start 的 POSIX 许可，再评估全局和连接规则。
3. 首次匹配拒绝发生在 worker permit/排队、凭据解析、DNS/TCP/SSH、job slot/记录/持久化之前。
4. 排队后在取凭据之前、最终 backend.execute/jobs.start 之前复查最新策略。
5. JobManager.start 的直接入口及后台 worker 的远端分发前均复用同一门禁。

初始拒绝必须验证：零凭据读取、零网络、零 backend 调用、零任务记录/槽位、零worker入队。允许读取必要的宿主授权/profile元数据，不混称零磁盘读取。

最后一次分发授权快照是并发时点。不跨网络持锁，不承诺撤销已经发生的远端副作用。新增命令规则不追杀正在运行的任务。排队后才被收紧的任务可记录 failed/command_denied，但不得执行远端命令。

只匹配用户 command，不匹配 Se 自己的 setsid/ps/kill 管理包装器。job_status/job_list/job_cancel 继续独立授权，不能因黑名单包含 kill/sh 而失去取消能力。客户端参数不能传 skipPolicy、confirmed 或其他绕过标志。

### 3. 错误与隐私

- 拒绝码建议 `command_denied`，固定消息「命令被宿主策略拒绝」，不自动重试。
- 无权限/身份变化优先于规则命中，避免未授权者探测规则。
- 非法配置返回 POLICY_INVALID 及安全字段路径/规则ID，不回显完整pattern或原始正则异常。
- JobData.finish 需为可信 command_denied 保留稳定错误，不能全部变成 ssh_error。
- 日志仅固定操作、revision和稳定code。禁止记录命令、pattern、匹配片段、命令hash、凭据或完整异常。

### 4. 界面与无副作用预览

SSH 分区增加「全局命令黑名单」，连接展开区增加「此连接追加规则」并只读展示继承的全局规则。规则编辑器提供匹配方式、内容、移除按钮，高级正则明确提示兼容范围。

提供「测试是否命中」预览，但**不执行命令**。前端只提交内存中的测试文本/草稿策略，由同一个 Rust evaluator 返回命中规则ID、来源、revision以及draft/persisted状态。「未命中」不等于「允许执行」，仍须展示操作授权阻塞原因。

建议等价管理接口 `mcp_host_access_preview` / `POST /mcp/host/access/preview`，沿用 LocalOperator/PublicTunnel 门禁，不开放修改授权的普通 MCP 工具。预览不解析凭据、不联网、不排队、不创建任务、不保存，测试文本不进localStorage、URL或analytics。

当前 `McpHostAccessPanel` 的 grant()/save() 会重建 `ssh: {profiles}`。实施全局字段时必须一并修正，否则普通授权编辑会丢失全局黑名单。新授权默认全部操作、只读预设、根目录修改、搜索和折叠均须保留规则及旧身份pin。

### 5. 迁移与实施位置

- `mcp_core/host_policy.rs`：v1→v2内存迁移、默认空规则、全量验证、编译快照、CAS与原子发布。保存成功后才切换生效revision。
- `ssh/automation.rs`：command参数前置，共享 authorize_command 门禁。
- `ssh/automation_jobs.rs`：直接start与worker复查、拒绝错误稳定化、最初拒绝无job副作用。
- `ssh/automation_backend.rs`：继续独立连接池，不读取第二份规则，不另写匹配器。
- `host_service.rs`、commands/web：共享preview与四端接线。
- shared类型、facade、UI、双语文案及插件能力目录同步。

旧策略不自动增加拒绝或授权。新 schema 降级时不得删掉规则继续运行，应明确备份和兼容流程。新增管理command/route必须同步能力目录，检查unclassified:0。

### 6. 验收矩阵

| 范围 | 重点 |
|---|---|
| 匹配 | exact/contains/regex，大小写、空白、引号、Unicode、换行、边界长度 |
| 配置 | 空pattern、非法正则、不支持语法、膨胀/嵌套/数量超限均整份拒绝 |
| 权限 | 全局与profile并集、不可取消全局拒绝、不同profile隔离、身份及POSIX仍生效 |
| 副作用 | execute/job_start/直接JobManager入口均拦截，初始拒绝零凭据/网络/排队/任务记录 |
| 并发 | 排队和凭据获取期间收紧规则，分发前复查，不追溯杀运行任务 |
| 控制面 | status/list/cancel不被内部包装命令误拦，已有撤权取消仍正常 |
| 保存/UI | 迁移、reload、损坏fail closed、revision conflict、编辑授权不丢全局规则 |
| 预览 | 同一Rust语义、仅内存草稿、不执行、不保存、不授予权限 |
| 跨端/隐私 | desktop/shared-live/se-server/browser/phone一致，公开隧道拒绝，日志sentinel不泄漏 |

## 备选

1. 更小首版仅提供 exact/contains，待有实际需求再开放高级regex，降低配置学习成本。
2. 若要防御恶意客户端，另做固定能力/命令白名单、远端最小权限账号或强制命令，这比扩大黑名单可靠。
3. 不采用前端 RegExp 作为权威、分隔字符串存规则、profile覆盖global或信任客户端预检结果。

## 不做

本轮不实现黑名单，不连接真实SSH，不改普通PTY或用户交互终端，不接管其他SSH客户端。不做完整shell解析、变量/base64/脚本语义分析、SFTP内容审查、完整危险命令集合或事后杀进程。

**文本黑名单是防误操作护栏，不是安全沙箱。** 引号拼接、变量、脚本、解释器或编码可能绕过朴素规则。独立SFTP write/upload还可修改授权目录中的脚本/配置，不经过命令匹配。必须配合窄文件根目录、独立文件授权与低权限远端账号。上述绕过边界来自静态分析，未做真实攻击实验。
