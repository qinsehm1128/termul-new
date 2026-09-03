# grok-bot 借鉴报告与落地方案(只读分析,未开发)

**Date:** 2026-09-01
**Scope:** 借鉴项 1(用量仪表盘)、3(MCP 反向桥)、4(composing 脉冲),并回答两个扩展问题:①能否不止 ACP、结合终端里跑的模型;②Rust 侧"知道每个模型价格的 router 项目"是否存在、如何用于成本匹配。
**参考仓库:** `/Users/qs/project/github/grok-bot-0.18-reconstructed`(下称 grok-bot)

---

## 0. 结论速览

| 问题 | 答案 |
|---|---|
| 终端里的模型能否纳入统计? | **能,且数据质量比 ACP 更细**。Claude Code 与 Codex 都在本机落盘了带 token 细分的 JSONL(已实测本机样本),新建一个 Rust 侧文件采集器即可纳入 |
| Rust 有没有"知道每模型价格"的项目? | 用户记忆的最接近对象是 **TensorZero**(Rust, Apache-2.0, 11.7k stars,**已归档**,最后 push 2026-06-11)——不建议依赖。**价格数据源应直接用 LiteLLM 的 `model_prices_and_context_window.json`(3408 模型,含 input/output/cache 单价)或 OpenRouter `/api/v1/models`(425 模型)**,两者均已实测抓取成功。crates.io 无成熟定价 crate(最大者 `llm-pricing` 仅 1.3k 下载的 CLI) |
| 三项借鉴是否成立? | 1 与 4 完全成立(有现成扩展点);3 成立(host_mcp 骨架可直接泛化),但需先补一个 PTY 元数据缺口 |

---

## 1. 调研证据

### 1.1 价格数据源(全部实测)

**LiteLLM 价格库**(推荐主源)
- URL: `https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json`
- 规模: 3408 个模型条目
- 字段(以 bedrock claude 条目为例):
  - `input_cost_per_token` / `output_cost_per_token` (USD/token)
  - `cache_creation_input_token_cost` / `cache_read_input_token_cost`
  - `max_input_tokens` / `max_output_tokens` / `max_tokens`
  - `litellm_provider` / `mode`
- 特点: 单 JSON 全量快照,可打包进应用内 + 定期刷新;键名与 Anthropic/OpenAI 官方模型名基本一致

**OpenRouter API**(推荐辅源/动态刷新)
- URL: `https://openrouter.ai/api/v1/models`(无需鉴权,实测 425 模型)
- 字段: `id`、`pricing.prompt` / `pricing.completion` / `pricing.input_cache_read`(USD/token, 字符串小数)、`context_length`
- 特点: 在线 API,价格最"新",但只覆盖 OpenRouter 在售模型

**TensorZero**(用户记忆中的项目,确认存在但不可依赖)
- `github.com/tensorzero/tensorzero`: Rust 编写的 LLM gateway,统一多 provider 接口 + 按模型配置定价并跟踪成本;Apache-2.0, 11.7k stars
- **`archived: true`**(仓库已归档)→ 不引入其 gateway;它的"模型定价配置 + usage→成本换算"思路已被上两个数据源替代
- crates.io 检索 `llm cost pricing`:无主导性 crate(`llm-pricing` 1.3k 下载、`openai-cost`/`gemini-cost` 各 24-25 下载),自建轻量模块比引依赖更合适

### 1.2 终端 CLI agent 的本地用量落盘(本机实测样本)

**Claude Code** — `~/.claude/projects/<编码后 cwd>/<session-uuid>.jsonl`
- 每条 assistant 消息记录:
  ```json
  "usage": {"input_tokens": 2, "cache_creation_input_tokens": 41335,
            "cache_read_input_tokens": 0, "output_tokens": 7,
            "output_tokens_details": {"thinking_tokens": 0}}
  "model": "claude-sonnet-5"
  "timestamp": "2026-08-01T04:30:53.161Z"
  ```
- 目录名即编码后的项目 cwd → **天然可与 termul 的项目/worktree 关联**
- 粒度: 逐条消息,input / cache 写 / cache 读 / output / thinking 全细分(比 ACP 事件还细)

**Codex CLI** — `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`
- `turn_context` 事件: `"model": "gpt-5.3-codex"`、`cwd`
- `event_msg` / `token_count` 事件:
  ```json
  "total_token_usage": {"input_tokens": 38265, "cached_input_tokens": 3456,
                        "output_tokens": 376, "reasoning_output_tokens": 238,
                        "total_tokens": 38641}
  ```
- 注意: `total_token_usage` 是**累计值**,需做差分;部分早期事件 `info: null` 需容忍

**结论:** "结合终端里面的模型"不需要任何 hack——两个主流 CLI 都有稳定的本地账本,按文件 tail + 偏移量采集即可,与 ACP 源并存互不干扰。

### 1.3 termul 侧对接点(file:line)

| 对接点 | 现状 | 证据 |
|---|---|---|
| ACP usage 事件 | `SessionUpdate::UsageUpdate` 解出 `used`/`size`/`cost.amount`,**无 input/output/cache 细分** | `src-tauri/src/acp/client.rs:376`、`src-tauri/src/acp/events.rs:641` |
| usage 存储 | renderer `sessionUsage: Record<SessionId, SessionUsage>`,仅会话内展示;`session_payload.rs` 有持久化累积器 | `src/renderer/stores/acp-store.ts:397/6367`、`src-tauri/src/acp/session_payload.rs:404` |
| PTY 元数据 | `SpawnOptions` 有 `program`/`args`/`kind`,但 **`TerminalInfo` 不下发这三者**——识别"终端里跑的是哪个 CLI"需补此缺口 | `src-tauri/src/pty/manager.rs:655/473/1901` |
| MCP 反向桥骨架 | host_mcp 已是"parent TCP(127.0.0.1:0 + token) ↔ child rmcp stdio server"架构,`#[tool]` 宏可加新工具;仅缺通用 FrameKind 路由 | `src-tauri/src/acp/host_mcp/mod.rs:39/155`、`parent.rs:103/186`、`child.rs:107/231` |
| 流式提交 | 正常流 rAF coalesce(每帧 ≤1 次 set),replay 路径 `useCoalesce=false` 逐 chunk set;`finalizeStreaming` 清 streaming 标记 | `src/renderer/stores/acp-store.ts:6111/6132/3534/894/1471` |
| Settings 扩展点 | 分类注册 `settings-categories.ts` + `AppSettings` 字段 + search index 同步 | `src/renderer/pages/AppPreferences.tsx` |

### 1.4 grok-bot 对应实现(借鉴蓝本)

| 借鉴项 | grok-bot 实现 | 要抄的设计点 |
|---|---|---|
| 用量累加器 | `SandInferenceRouterUsageProvider { requests, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, lastUsedAt }`,schemaVersion 1,`recordInferenceUsage()` 原子累加,存 settings.json | schema-versioned、按 provider 分账、原子累加、UI 免责标注("活动记录而非权威账单") |
| MCP 桥 | 本地 HTTP server `127.0.0.1:{random}/mcp/{uuid}`,`tools/list`/`tools/call` JSON-RPC,工具按名字自动标 `readOnlyHint` | loopback + 随机端口、注解自动标注、把自家工具暴露给外部 CLI agent |
| composing 脉冲 | 发送前 `beginActivity()` 置 thinking + `isComposingMessage`,**1200ms 保活**防虚拟滚动闪烁,`finally endActivity()` 清除 | 状态最小展示时长;同时覆盖 error/crash 分支的清除 |

---

## 2. 方案 A:用量仪表盘(借鉴项 1 + 两个扩展问题的落点)

### 2.1 目标形态

设置页新增 **Usage** 分类:按天/按项目/按模型/按来源(ACP 会话、终端里的 Claude Code、终端里的 Codex)展示 requests、token 细分与估算成本;终端 Board 可选一个轻量"本月花费"徽标。

### 2.2 数据结构(Rust 侧,新增 `src-tauri/src/usage/`)

```rust
// 一条不可变流水记录;append-only JSONL(仿 ACP session_persistence 的 versioned schema 做法)
#[serde(tag = "v")]
pub struct UsageRecordV1 {
    pub ts: String,                    // RFC3339
    pub source: UsageSource,           // Acp | ClaudeCode | Codex
    pub project_cwd: Option<String>,   // ACP 会话取 session cwd;CLI 源取其落盘 cwd/目录名
    pub terminal_id: Option<String>,   // PTY 会话 id(能关联时)
    pub session_ref: Option<String>,   // CLI 的 session-uuid / rollout 文件 id
    pub model: Option<String>,         // 原始模型 id,如 claude-sonnet-5 / gpt-5.3-codex
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,         // 匹配到价格时写入;匹配不到留空(诚实原则)
}

pub struct PricingEntry {
    pub input_per_token: f64,
    pub output_per_token: f64,
    pub cache_read_per_token: Option<f64>,
    pub cache_write_per_token: Option<f64>,
    pub context_window: Option<u64>,
}
```

**模型 → 价格匹配策略**(三级,全部 miss 则 `cost_usd = None`):
1. 精确 id 匹配(LiteLLM 键与官方模型名一致,`claude-sonnet-5`、`gpt-5.3-codex` 均可直查);
2. 归一化匹配(去日期后缀/provider 前缀,如 `anthropic/claude-sonnet-5` ↔ `claude-sonnet-5`);
3. 人工映射表(用户可在设置里补充自定义映射)。

**价格表模块**:应用内打包一份 LiteLLM 快照(冷启动可用),设置里提供"从 LiteLLM/OpenRouter 刷新"按钮(在线更新)。不引 TensorZero gateway,不引 crates.io 依赖。

### 2.3 三源采集

| 源 | 采集方式 | 关键点 |
|---|---|---|
| ACP 会话 | 现有 `EVENT_USAGE_UPDATE`(`events.rs:380`)事件流直接落账;**缺口**: 事件只有 `used/size/cost`,若上游 agent 回送更细 usage 需扩 `UsageUpdateEvent` | 与 `session_payload.rs:404` 的持久化累积器共用入口,避免双写 |
| 终端 Claude Code | 新增 `usage/ingest/claude_code.rs`:tail `~/.claude/projects/**/*.jsonl`,按文件偏移量增量解析 assistant 行 | 目录名解码即 cwd;偏移量状态持久化,防止重复计数;容忍格式漂移(未知行跳过) |
| 终端 Codex | 新增 `usage/ingest/codex.rs`:tail `~/.codex/sessions/**/*.jsonl`,`token_count.total_token_usage` **做差分**(累计值) | `info: null` 事件跳过;`turn_context.model` 与最近 usage 行按 turn 关联 |
| PTY 关联 | 扩展 `TerminalInfo` 下发 `program`(`manager.rs:473`),spawn 时已知 `claude`/`codex`(`manager.rs:655`) | 仅用于展示"这个终端在跑什么",不是采集的前提(文件源已自带 cwd) |

**去重**:同一模型若同时被 ACP 托管与终端直跑,两者落盘路径不同(`~/.claude/projects/` 里的 acp-probe 目录 vs 正常项目目录),按 `source + session_ref` 天然幂等;采集器以 `(file_id, byte_offset)` 为水位线。

### 2.4 UI

- `settings-categories.ts` 注册 `Usage` 分类(平台门控沿用现有机制)+ search index 同步;
- 面板内容:日/项目/模型/来源四个维度的聚合表 + 成本列(miss 时显示 "—");
- 顶部沿用 grok-bot 的诚实标注:"本地面板的活动记录,非权威账单";
- 数据流:Rust 侧聚合查询命令(按日/项目/模型 group by),避免把全量流水推给 renderer。

---

## 3. 方案 B:MCP 反向桥(借鉴项 3)

### 3.1 目标形态

termul 把自身能力(终端会话、Skills、worktree、文件检索)作为 MCP server 暴露,**给跑在 termul 终端里的外部 CLI agent**(Claude Code / Codex)调用——反向于现有 `acp_probe_mcp_server`(那是 termul 去测别人)。

### 3.2 复用 host_mcp 架构,不新造 transport

现有骨架已给出全部要件:

| 现有件 | 复用方式 |
|---|---|
| `parent.rs:103` HostPlanServer:127.0.0.1:0 TCP + bearer token(`TERMUL_PLAN_TOKEN` 模式) | 泛化为 TermulBridgeServer:同一监听器加 `FrameKind` 分支(`mod.rs:155` 的枚举加 Terminal / Skill / Worktree 等变体) |
| `child.rs:107` rmcp `#[tool]` stdio server + watchdog(`child.rs:231` 防悬挂) | 新增一套 `#[tool]`:如 `list_terminals`、`send_keys`、`read_terminal_output`、`search_files`、`list_skills`;stdio→TCP 转发模式不变 |
| 对外注册 | 用户执行一次 `claude mcp add termul -- <termul bridge child 命令>`(或 Codex 等价配置),child 即以 stdio 服务该 CLI;token 经 env 注入,与 plan 工具同一安全模型 |

### 3.3 从 grok-bot 抄的两个细节

1. **工具注解自动标注**(`routed-mcp-bridge.ts:69`):读类工具(`list/read/search/get`)自动 `readOnlyHint=true, destructiveHint=false`,写类相反——让外部 agent 的权限提示更准确;
2. **loopback + 随机端口 + 每会话 token**:termul 现有模式已满足,保持"bind 127.0.0.1、token 走 env、child 有 watchdog"三件套。

### 3.4 前置缺口

- `TerminalInfo`(`pty/manager.rs:473`)需补 `program` 字段下发,否则 `list_terminals` 无法告诉 agent "哪个 tab 在跑什么";
- 工具清单第一版建议只放只读类(读终端输出、列 Skills、列 worktree),写类(往 PTY 发键)放到权限确认之后再开。

---

## 4. 方案 C:composing 脉冲(借鉴项 4,最小改动)

对齐 grok-bot 的"状态最小展示 1200ms"思想,termul 有三个具体落点:

1. **activeTurn 最小保持**:`scheduleTurnEnd`(`acp-store.ts:1471`)的 setTimeout 分支竞态下 activeTurn 可能先清再恢复导致 spinner 闪烁 → 给 `finalizeStreaming`(`acp-store.ts:894`)加 1200ms 最小保持:若距 `activeTurn` 置位不足 1200ms 则延迟清除;
2. **replay 逐 chunk set**:`useCoalesce=false`(`acp-store.ts:6132`)使 session/load 回放期间逐 token 渲染 → replay 也走 coalesce,只是 flush 时机改为同步批量;
3. **思考期空气泡**:新 message `streaming: true` 后若长时间无可见 block,给气泡渲染一个统一的 "thinking" 脉冲占位(对应 grok-bot 的 `isComposingMessage` + 脉冲),首个 block 到达前保持稳定。

改动全部局限在 `acp-store.ts` 与气泡组件,不触 Rust 侧。

---

## 5. 分阶段实施顺序建议(本次不开发)

| 阶段 | 内容 | 依赖 | 规模 |
|---|---|---|---|
| P0 | 方案 C(composing 脉冲,3 个小修) | 无 | 最小,先拿手 |
| P1 | usage 骨架:`UsageRecordV1` 流水 + ACP 源落账 + LiteLLM 快照定价匹配 + Settings Usage 面板 | 无 | 中 |
| P2 | 终端源采集:claude_code / codex 两个 ingest + `TerminalInfo.program` 下发 | P1 | 中 |
| P3 | MCP 反向桥:泛化 host_mcp FrameKind + 只读工具集 + `claude mcp add` 接入文档 | 独立于 P1/P2 | 中偏大 |

两个 renderer 根(`TauriApp.tsx` / `App.tsx`)在各阶段保持一致;web/远程面(A2A)下 Usage 面板按 `isTauriContext()` 门控或经 web transport 等价暴露,遵循 AGENTS.md 的四面对照要求。

## 6. 风险与边界

- **CLI 落盘格式是私有契约**:Claude Code / Codex 的 JSONL 字段可能随版本漂移 → 解析器按"未知行/未知字段跳过"设计,水位线 + 测试样本钉版本;坏行不影响其余流水。
- **Codex 累计值差分**:进程重启或文件截断时差分基准失效 → 以文件为重置单位,`total < last` 时按新基准重置。
- **价格时效性**:LiteLLM JSON 是社区维护快照,新模型可能 miss → miss 时显示 "—" 而非估个错数(诚实原则,grok-bot 同款);提供刷新按钮。
- **cache token 语义差异**:Anthropic(cache_creation/read)与 OpenAI(cached_input)口径不同 → `UsageRecordV1` 存原始细分,换算规则放在定价模块内按 provider 分派。
- **TensorZero 已归档**:只作思路参考,不引代码、不加依赖。
- **用量隐私**:全部数据本地产物、本地聚合,不新增任何遥测(符合项目"不记密钥/凭据"的日志纪律)。

## 7. 明确不做

- 不引入 TensorZero gateway(已归档、层次不对:termul 不需要自建 LLM 网关,只需要价格表);
- 不做 per-provider 直连 API(grok-bot 的 AI 层正是反面教材);termul 保持 ACP 为主干;
- 不借鉴 grok-bot 的 secrets 分槽(单用户场景未出现需求)、reactions、binding manifest 逆向工程设施。
