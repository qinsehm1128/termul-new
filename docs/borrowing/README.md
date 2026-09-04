# 优化借鉴目录(Borrowing Research)

外部参考项目的对抗性评估与借鉴方案。所有结论均为只读分析,未做任何开发;参考仓库 checkout 在 `reference/`(gitignore,不进版本库)。

## 文档清单

| 文档 | 参考项目 | 主线内容 |
|---|---|---|
| [grok-bot-borrowing-plan.md](./grok-bot-borrowing-plan.md) | grok-bot-0.18-reconstructed | 用量仪表盘(ACP + 终端 CLI 账本 + 定价匹配)、MCP 反向桥、composing 脉冲 |
| [thinkrail-borrowing-plan.md](./thinkrail-borrowing-plan.md) | JetBrains/thinkrail | Composer 三态发送、终端 reserve/attach 三件套、Review 锚定、gh 一键 PR、悬空 toolCall 修复 |
| [terminal-renderer-dom-fallback.md](./terminal-renderer-dom-fallback.md) | JetBrains/thinkrail(渲染层专篇) | WebglAddon 弃用证据、崩溃机制对比、DOM 回退 A/B 实验设计 |
| [nyaterm-borrowing-plan.md](./nyaterm-borrowing-plan.md) | nyakang/nyaterm | SSH 远程系统监控采样器、主题导入导出/自定义主题、终端主题纹理刷新 |
| [obsidian-doc-capability-feasibility.md](./obsidian-doc-capability-feasibility.md) | 多项目调研 | Obsidian 式文档能力(vault / [[wiki链接]] / backlinks)+ WebDAV 同步 + 本地向量化的实现路径与可借鉴项目清单 |

## 合并优先级视图

| 优先级 | 项 | 来源 |
|---|---|---|
| 高 | composing 脉冲(最小改动) | grok-bot |
| 高 | 用量仪表盘三源(ACP / Claude Code / Codex + LiteLLM 定价匹配) | grok-bot |
| 高 | Composer 三态发送 + QueueStrip | ThinkRail |
| 高 | 远程主机资源面板(SSH 内嵌脚本 + 两帧差分采样器;本地面用 sysinfo 补位) | NyaTerm |
| 中 | 终端渲染层 DOM 回退 A/B(对应 leftover glyph 调查) | ThinkRail |
| 中 | 主题导入导出 + 自定义主题 + 终端主题纹理刷新触发 | NyaTerm |
| 中 | 终端 reserve/attach + prebind 缓冲 + 抢占通知 | ThinkRail |
| 中 | MCP 反向桥(host_mcp 泛化) | grok-bot |
| 中 | Review 评论锚定闭环;gh CLI 一键 PR;悬空 toolCall 修复 | ThinkRail |
| 低 | Default workspace 锚、external worktree 附加、compaction 可见性、per-window 主题 | ThinkRail / NyaTerm |

## 共同纪律

- 对抗性筛选:已具备或更优的能力不借鉴(termul 在会话持久化、流恢复、ACP 子进程隔离上保持领先);
- 参考仓库只读,借"模式与边界"不盲目搬代码;如需引用代码片段,注意各仓库 license(Apache-2.0 / MIT)并带 NOTICE;
- 借鉴项落地时遵循 AGENTS.md 四面对照(Tauri 桌面 / shared-live 远程 / standalone server / 浏览器)与双 renderer root 一致性。
