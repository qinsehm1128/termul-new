# JetBrains ThinkRail 借鉴报告(只读分析,未开发)

**Date:** 2026-09-01
**参考仓库:** `/Users/qs/project/me/termul/reference/thinkrail`(git clone @ JetBrains/thinkrail,已 gitignore)
**关联文档:** [`./grok-bot-borrowing-plan.md`](./grok-bot-borrowing-plan.md)(用量/价格主线在彼处,本文不重复)

## 0. ThinkRail 是什么

JetBrains 开源(Apache-2.0)的 **Worktree IDE**:围绕 `pi` coding agent 的一层"瘦宿主"——pi 进程内运行并拥有模型/技能/压缩/成本/会话状态;app 拥有工作区、编辑器与传输线。三层架构:engine host(Bun.serve HTTP+WS)→ types-only wire(`packages/contracts`)→ mobile-first React 客户端(`apps/web`),桌面(Electrobun)与 CLI 只是同一 host 库的两个薄 launcher。与 termul 的定位(终端工作区 + AI 会话 + worktree + 浏览器客户端)高度重叠,是迄今对标度最高的参考项目。

## 1. 借鉴清单(按对抗性筛选排序)

### ✅ A. Composer 三态发送 + 队列(termul 真实空白,价值最高)

ThinkRail 在 agent 流式运行中区分三种投递:`steer`(Enter,下一 step 生效)/ `followUp`(⌘+Enter,排队等 agent 结束)/ `interrupt`(⌘+Shift+Enter,中断后发送),Send 按钮三态化 + popover,placeholder 动态提示;`QueueStrip` 按 lane 渲染队列项,每项可编辑/删除。
证据:`apps/web/src/chat/Composer.tsx:51-70/378-400/282-293`、`QueueStrip.tsx:1-41`。
termul 现状:全库无 steer/followUp/interrupt(已 grep 确认),流式中只能等或手动停。ACP 会话加这套语义 + 队列条,是对话体验的直接升级。

### ✅ B. 终端连接语义三件套(对 shared-live 远程面尤其有价值)

1. **reserve/attach 分离**:`reserveTerminal()` 只注册 catalog tab(无进程、幂等),`attachTerminal()` 是 PTY 诞生的唯一途径且幂等重连 —— 刷新页面/换客户端不产生僵尸 shell。`terminalManager.ts:155-230`
2. **owner-scoped 抢占通知**:attach 时旧持有者收到 `TerminalDetachedPush`;write/resize 校验 owner。`terminalManager.ts:207-262`
3. **prebind 缓冲**:PTY attach 前先缓冲 frames(限 1MB/256 帧),bind 时一次回放,attach 前输出不丢。`apps/web/src/panels/terminalPrebindBuffer.ts:1-70`
另有 8ms/32KB 输出批处理 + backpressure 检测(`outputBatcher.ts:1-70`)。
termul 已有 workspace_manifest 的 worktree 感知重连(`acp/workspace_manifest.rs:66`),但"attach 前输出缓冲"与"被抢占通知"值得对照补齐——这正是远程/手机面最容易丢体验的地方。

### ✅ C. 悬空 toolCall 修复(server 重启鲁棒性)

`sessionRepair.ts:1-40`:host 重启后扫描会话,对悬空 toolCall(尤其 `ask_user_question`)注入 decline 回复,避免会话卡死。termul 已有 RecoveryItemV1 会话恢复,但"工具调用等待中"这一断点的修复未见对应物——建议对照 `session_persistence.rs` 补这一类。

### ✅ D. 工作区级 Review(termul 真实空白)

workspace 一个 JSON(`reviews.ts:1-220`):评论锚定用 contentHash + lineRange + textQuote 三重定位,内容漂移时 `reanchor()`(`anchoring.ts`);`ReviewSurface` 区分 file/diff 两种锚面;草稿评论批量"发送给 chat session"让 agent 消化(`SendReviewButton.tsx`);评论可打包成结构化输入给 agent(`packageRender.ts`)。
这是"人对 agent 产出做行级批注 → 喂回 agent"的闭环,termul 完全没有;与 grok-bot 报告里的 MCP 反向桥互补。

### ✅ E. host 侧一键 PR(经用户自己的 gh CLI)

`pr/pr.ts`:push 前刷新 worktree 分支防竞态、`GIT_TERMINAL_PROMPT=0` 非交互、先 `gh pr list --head` 查重 → 有则 `edit` 无则 `create`、失败后 re-check 防重复;gh 不可用时降级为 GitHub compare URL。**不存 token、不调 provider REST API**,纯借用户本地 gh 登录态——与 termul"复用 CLI 登录态"的既有哲学一致,termul 的 worktree 分支可直接挂此流程。`pr.ts:12-19/43-119/125-145`

### ⚠️ F. WebGL 终端渲染的反对证据(与 termul 现存 bug 直接相关)

ThinkRail **刻意不用** `addon-webgl`,理由:xterm 官方维护者称 DOM renderer 是触控前提;`WebglAddon.dispose()` 泄漏 WebGL2 context(对 per-worktree 终端高频创建销毁是致命的);iOS context 上限崩溃。架构文档把它写成一等决策并给出复评条件(ghostty-web 成熟度)。termul 的 [`leftover-glyphs-handoff.md`](../leftover-glyphs-handoff.md) 正在调查 WebGL leftover glyph 问题——建议把这份反对证据纳入该调查的备选方向(DOM renderer 兜底),而不是继续只在 WebGL 内打转。`architecture.md` 决策 #11、`apps/web/src/panels/TerminalInstance.tsx`

### ⚠️ G. 工作区模型的两个 UX 锚(部分借鉴)

- **Default workspace**:每个 project 一个不可删改的"项目文件夹本身"工作区(`kind:"default"`),Welcome 三卡入口之一是"Work in project folder"——给在 worktree 模型里迷路的用户一个锚。termul CAP-3 worktree 已存在,缺的是这个显式锚 + Welcome fork。`goal-and-requirements.md:23-30`、`WelcomePanel.tsx:57-60`、`defaultWorkspace.ts`
- **external worktree 附加**:用户已有 worktree 可原位挂入(`kind:"external"`,只 forget 不 mutate)。termul worktree 若只支持自建,可补此入口。`ExistingWorktreeDialog.tsx`

### ⚠️ H. Compaction 可见性

压缩后 transcript 插入专用 turn `{kind:"compaction", summary, tokensBefore}`,UI 展示压缩前 token 量(`hydrate.ts:51-57`)。termul/pi 压缩目前对用户不可见——低成本高感知的透明度改进。

### 📋 I. 工程纪律(机制可抄,YAML/配置不抄)

1. **exact-pin 强制**:所有依赖钉死精确版本,`scripts/check-catalog.ts` 在 pre-commit + CI 拒绝任何 range/catalog 漂移(`architecture.md` 决策 #10)。termul 可加等价的 npm CI 检查;
2. **types-only wire + `protocolVersion` 协商**:`ServerWelcome` 携带协议版本,独立发布的 web UI 可检测 host 漂移(`wsProtocol.ts:100-107`)——termul web/远程面适用;
3. **spec-first 流程**:每模块 co-located `SPEC.md` + frontmatter 互链 + spec_* 工具 + 只读 Specs 面板;改边界必须同改 spec。这是流程借鉴,不是功能;
4. **Analytics 隐私纪律**(若 termul 将来要埋点的黄金标准):仅 5 个白名单事件、provider/model 归一化桶、CI/test 环境静音、`gatedFetch` 在开关关闭时拦截、personless + GeoIP off。`analytics/events.ts:10-32`、`mute.ts`、`sink.ts`

## 2. 明确不借鉴

- **进程内 agent 无崩溃隔离**(`architecture.md` Invariants):致命故障拖垮整个 host,是他们自认的取舍;termul 的 ACP 每会话独立子进程 + 驱动线程(`acp/manager.rs`)是更稳的模型,保持;
- **Electrobun/自装 launcher 与 bun-pty**:termul 是 Tauri + portable-pty,生态不同,无迁移理由;
- **tmux 拒绝决策的反面**:他们因 Windows 不可假设而拒绝 tmux 作持久层——termul 的"跨项目切换保留 PTY"红线(AGENTS.md 已知坑)与其结论一致,无需动作;
- **PostHog 接入本身**:termul 无遥测是现状,除非产品决策变化,否则不引入。

## 3. 与 grok-bot 报告的合并视图

| 借鉴项 | 来源 | 优先级建议 |
|---|---|---|
| Composer 三态发送 + QueueStrip | ThinkRail | 高(对话核心体验,独立小改) |
| 用量仪表盘 + 终端 CLI 账本 + 定价匹配 | grok-bot 报告 P1/P2 | 高 |
| composing 脉冲 | grok-bot 报告 P0 | 高(最小) |
| 终端 reserve/attach + prebind 缓冲 + 抢占通知 | ThinkRail | 中(远程面价值最大) |
| Review 评论锚定闭环 | ThinkRail | 中(新功能面) |
| gh CLI 一键 PR | ThinkRail | 中(依赖 worktree 流程成熟度) |
| 悬空 toolCall 修复 | ThinkRail | 中(鲁棒性补丁) |
| MCP 反向桥 | grok-bot 报告 P3 | 中 |
| Default workspace 锚 / external worktree / compaction 可见性 | ThinkRail | 低(锦上添花) |

## 4. 风险与边界

- ThinkRail 的 wire/审批模型绑定 pi 生态;termul 走 ACP,借鉴时语义需映射(如 steer/followUp 需确认 ACP 层能力边界——pi 的 ACP 变体是否暴露同语义待实现前验证);
- 参考仓库为 gitignore 的本地 checkout,不进版本库、不引其代码(只读参考,license 为 Apache-2.0,如需借代码片段须带 NOTICE);
- 报告基于 architecture.md + 定向代码勘察,`session.list` 版本协商细节与 analytics schema 两处未深挖(探索代理标注 not found),落地前需再核。
