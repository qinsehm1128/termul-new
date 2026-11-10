---
schema: qin-doc/1
kind: proposal
id: seremote-terminal-centric-ia
title: SeRemote 以终端为中心的信息架构
date: 2026-11-10
status: draft
source: agent://ia-terminal; user 2026-11-10 13:4x/13:54
lang: zh-CN
artifact: {"work_id":null,"created_by_session":"session_rooster_1794152096588_1fe993dfae702a29","last_updated_by_session":"session_rooster_1794152096588_1fe993dfae702a29","session_ids":["session_rooster_1794152096588_1fe993dfae702a29"],"todo_refs":[],"goal_refs":[]}
---

## 问题

- 用户原话：「从文件里面进去的这些操作和界面,我觉得没有什么意义,应该直接围绕终端来做吧?现在很多功能我觉得都很乱」。
- 证据来自 `agent://ia-terminal`，全部是静态核对，模拟器未运行。
- 乱的根源：原型（`.qin-code/design/seremote-ios27-v2/`）之外多出了四个"地方"。
  - Dashboard 主机目录：一个 fullScreenCover，里面的点击都在遮罩后生效（`ConnectedShellScreen.swift:132-135`、`HostHomeView.swift:115-121,220-228`）。
  - 独立的快捷终端控制台：终端页行点击进的是 `TerminalPTYViewer`，不是工作台的 `TerminalScreen`（`TerminalsHubScreen.swift:203-208`）。
  - 终端内又嵌一层「终端/会话」选择（`TerminalAgentChatContainer.swift:47`）。
  - 文件被做成一个目的地：有 5 个入口，终点是只读预览，只能复制或分享（`WorkspaceFilePreviewView.swift:195-215`）。
- 其他重复：
  - 桌面有 6 个入口、4 个 sheet 实例。
  - 会话和项目列表有 3 份。
- 失效点：`spawnTerminal()` 在 `workspace == .home` 时静默返回（`WorkspaceSession.swift:479-481`）。

## 方案

推荐方案 A：以终端为家。分两步实施，先做 C 的减法，再做 A 的收口。

```
主机列表
└─ 已连接 → 终端（默认落地：上次的终端）
   顶栏：项目·终端标题(点开切换器) / 文件 / 对话 / 桌面 / ⋯
   ├ 文件面板：当前终端 cwd → 预览 → 插入路径 / 发给 Agent / 复制 / 分享
   ├ 对话面板：当前终端的 hook 会话（无 hook 时显示空状态）
   ├ 桌面 sheet：取景截图 → 当前终端或 Agent
   └ ⋯：新建终端 / 重命名 / 工具 4 格(作用域=当前项目)
底栏：终端 | 会话 | 工具；设置在抽屉
iPad：侧栏(主机+按项目分组的终端) | 终端 | 右侧检查器(文件/对话)
```

- 锁定问题采用默认答案：
  - Q1：文件不再是目的地。
  - Q2：连上主机直接进终端。
  - Q3：快捷终端控制台和工作台终端合并成一个表面。
  - Q4：桌面入口从 6 个收到 2 个，即终端 `⋯` 和屏幕页。
  - Q5：工具作用域是当前项目，没有项目时禁用。
  - Q6：对话作为终端的面板，会话页只列代理会话。
- 不变量 I1–I5 必须保留：
  - I1：终端子树常驻，用 opacity 切换。
  - I2：切模式只处理几何租约，不结束 PTY。
  - I3：RootView 按 `link.id` 重建。
  - I4：快捷终端租约隔离。
  - I5：离开工作台不能销毁主机侧 PTY。
- 流程：先出排版和 HTML 原型（`.qin-code/design/seremote-terminal-centric/`），用户确认后才改 iOS 代码。

### 排版（手机竖屏）

共 9 屏。复用 v2 原型的组件：`navBar`、`tabstrip`、`termmeta`、`term`、`keyRow`、`cmdline`、`grouplabel`、`termRow`、`qrow`、`ribbon`、`perm`、`composer`、`toolcard`、`groupcard`/`rowitem`、`sheet-scene`/`scrim`。

```
T1 终端（底栏第一项，连上后的落地页）
┌ 终端            MacBook Pro · 5 个终端     [↻] ┐
│ ┌ 继续 ─────────────────────────────────────┐ │  上次终端，点即进 T2
│ │ ● zsh — Termul   main · ~/project/termul ›│ │
│ └───────────────────────────────────────────┘ │
│ Termul 03                                     │
│  zsh — Termul   main · ~/project/termul  Agent│  行点击 → T2（唯一终端表面）
│  dev server     main · ~/project/termul       │
│ se-server 01 / 其他 01                        │
│ 快捷终端 02     zsh — Termul        [打开][⋯] │  打开 → 同一个 T2
│ + 新建终端                                     │
└ 底栏：终端 | 会话 | 工具 ──────────────────────┘

T2 终端工作台（核心）
┌ ‹  zsh — Termul ▾                  [▤][🖥][⋯] ┐  ▤ 文件 → T3；🖥 桌面 → T5
│    MacBook Pro · termul · main                │  标题▾ = 同项目终端切换
│ [1 zsh][2 dev][3 test][+]                     │
│ ● PTY · Ready  80×24           [FIT][A+]      │
│ ┌ ● Claude Code · 工作中 · 1 个许可    对话 › ┐│  仅有 hook 会话时出现 → T4
│ ┌ 终端（吃满）                               ┐│
│ └───────────────────────────────────────────┘│
│ Esc Tab Ctrl ↑ ↓ ← → … 粘贴        [隐藏键盘]│
│ $ 输入命令，或按住麦克风                [↑]  │
└───────────────────────────────────────────────┘

T3 文件面板（sheet，从 T2 升起，终端仍在背后）
┌ termul / src / net                       [✕] ┐  根 = 当前终端 cwd
│  ..  Sources  Package.swift  README.md …     │
│ ─ 预览 TerminalSocket.swift · 342 行 ──────── │  md/html/图片同现有渲染
│ [插入路径]  [发给 Agent]  [复制]  [分享]       │  主操作：插入路径 → 写入 T2 输入
└───────────────────────────────────────────────┘

T4 对话面板（sheet large）
┌ Claude Code · Sonnet 4.5      ● 工作中   [✕] ┐
│ 消息 / readchips / 许可卡（允许一次 / 拒绝）   │
│ composer：快捷回复 + 输入框 + 发送             │
└───────────────────────────────────────────────┘
  无 hook 时 ⋯ 里显示「对话」置灰 + ⓘ（在电脑端 Hooks 安装）

T5 桌面 sheet
┌ 桌面 · 主显示器                           [✕] ┐
│ ┌ 实时画面 + 取景框（拖动选区）            ┐ │
│ 目标：zsh — Termul ▾                          │  默认：对话就绪→Agent，否则→终端
│ [发送选区]（主）  [整屏]                       │
└───────────────────────────────────────────────┘

T6 ⋯ 菜单（T2 右上）
  新建终端 / 重命名 / 对话 / 屏幕文本 / 更改 / 搜索 / 定时任务
  ── 关闭终端（红，二次确认：「关闭后该 shell 会结束」）

T7 会话（底栏第二项）
  待你处理细带（有才显示）+ 代理会话列表
  点会话：有终端 → T2 + 自动打开 T4；无终端（ACP）→ 对话全屏

T8 工具（底栏第三项）
  主机切换 + 4 格（更改/定时任务/搜索/屏幕，作用域=当前项目）
  最近项目 → 该项目在 T1 的分组展开；设置
```

### 排版（iPad）

- 横屏 1194×834：250pt 侧栏（主机 + 按项目分组的终端 + 会话 + 工具）| 中间 T2 | 320pt 右侧检查器（分段：文件 / 对话，可收起）。
- 竖屏 834×1194：76pt 图标轨道 | T2 | 检查器改为从右侧滑出的面板。
- 桌面始终是 sheet。

## 备选

- **B：项目 → 终端 → 终端。**
  - 终端是唯一的全屏表面，文件和终端同屏分栏，会话是第二入口。
  - 改动 14–18 个文件。
  - 代价：需要显式建立"会话对应哪个终端"的绑定。
- **C：只做减法。**
  - 删除 Dashboard、终端内 Picker 和第二套控制台；桌面入口收到 2 个，文件入口收到 1 个；预览加出口动作。
  - 改动 10–13 个文件，风险中低。
  - 代价：文件仍是一个"地方"，围绕终端只做到一半。

## 不做

- 不删除文件的渲染能力（Markdown、HTML 沙箱、图片）。
- 不改 P1 协议，不改 PTY 保活结构。
- 原型确认前不改 iOS 代码。
- 「桌面打开文件」需要 host 侧能力，往后放。
