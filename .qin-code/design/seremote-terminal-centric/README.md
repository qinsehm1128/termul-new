# SeRemote · 以终端为中心

可点的静态原型，对应提案 `docs/proposals/seremote-terminal-centric-ia-proposal.md` 的 T1–T8 与 iPad 两向。
一个自包含的 `index.html`（CSS/JS 内联，沿用 v2 的 token、组件与图标），加 `shots/` 截图。

## 打开

| 目的 | 地址 |
|---|---|
| 展板（全部屏 + iPad） | `index.html` |
| 单屏 | `index.html?solo=<id>&still=1` |
| 日间 | 追加 `&mode=day`；或点展板右上「日间」 |
| iPad 横 / 竖 | `?solo=pad&o=ls` / `?solo=pad&o=pt` |

`solo` id：`home` `terminals` `wterm` `wterm-nohook` `files` `files-md` `chat` `desktop` `more` `more-confirm` `conversations` `tools` `settings` `git` `tasks` `search` `screen` `drawer`。

## 与 v2 的差异

**移除**

- 三模式开关（会话 / 终端 / 文件 分段）：终端不再被模式切走，文件与对话改为终端上的 sheet。
- Dashboard 式首页（主机 → 会话 → 工具）：连接后直接落在终端。
- 终端内的会话选择器（带侧栏的会话列表）：hook 会话只保留一条细带。
- 独立的 console 屏与 `/console` 入口。
- 桌面入口由 6 个减到 2 个：标题栏的文件、桌面。

**新增**

- 落地页的「继续」hero、项目分组、快捷终端、新建终端。
- 终端标题栏：`⌄` 同项目终端切换、文件、桌面、⋯。
- hook ribbon（`Claude Code · 工作中 · 1 个许可 · 对话 ›`）；无 hook 时整条消失。
- 文件 sheet：面包屑根在终端 cwd，内联预览，四个出口（插入路径为主操作）。
- 桌面 sheet：取景框、目标终端、发送选区 / 整屏。
- 更多动作表 + 关闭确认（`关闭后该 shell 会结束`）。
- iPad：250 侧栏 + 中栏 T2 + 320 检查器（文件 / 对话）；竖屏 76 轨道 + 右侧滑入检查器。

## 屏幕

| # | id | 屏 | 说明 |
|---|---|---|---|
| 01 | `home` | 主机 | 未连接主机列表，沿用 v2；连接后去终端 |
| 02 | `terminals` | 终端 | T1 落地：继续、项目分组、快捷终端、新建终端；底栏三格 |
| 03 | `wterm` | 工作台 | T2：标题栏 + 标签 + PTY + ribbon + 全高终端 + 键盘行 + 命令行 |
| 04 | `wterm-nohook` | 工作台 | 同一屏，无 hook ribbon |
| 05 | `files` | 文件 | T3：sheet over T2，cwd 面包屑、行、Swift 预览、四个动作 |
| 06 | `files-md` | 文件 | 同一 sheet，Markdown 预览 |
| 07 | `chat` | 对话 | T4：大 sheet over T2，气泡、读取 chip、许可卡、composer |
| 08 | `desktop` | 桌面 | T5：CSS 桌面、取景框、目标终端、发送选区 / 整屏 |
| 09 | `more` | 更多 | T6：七个动作 + 红色关闭终端 |
| 10 | `more-confirm` | 更多 | 关闭确认，只写后果 |
| 11 | `conversations` | 会话 | T7：待你处理 + 代理会话列表；行回到 T2 + 对话 |
| 12 | `tools` | 工具 | T8：更改 / 定时任务 / 搜索 / 屏幕文本 + 最近项目 + 设置 |
| 13–16 | `git` `tasks` `search` `screen` | — | 沿用 v2，返回目标是 `wterm` |
| 17 | `settings` | 设置 | 主题、通知、字号、凭据 |
| 18 | `drawer` | 抽屉 | 终端 / 会话 / 工具 + 主机 + 设置 |
| 19 | pad ls | iPad 横 | 1194 × 834：250 + T2 + 320 |
| 20 | pad pt | iPad 竖 | 834 × 1194：76 轨道 + T2 + 滑入检查器 |

## 已连的流程

- T1 行 / hero → T2
- T2 文件图标 → T3；T3「插入路径」→ T2，命令行填入 `src/net/TerminalSocket.swift` + toast
- T2 ribbon → T4；T4 ✕ → T2；T7 行 / 待你处理 → T2 + T4
- T2 桌面图标 → T5；「发送选区」/「整屏」→ T2 + toast
- T2 ⋯ → T6；「关闭终端」→ 确认；确认后 → T1 + toast；「对话」→ T4
- T2 标题 `⌄` → 终端切换；标签页切换标题
- 底栏 终端 / 会话 / 工具；抽屉 → 三处 + 设置
- T4 许可卡「允许一次」→ 已允许一次

## 截图

在 `shots/`，用 headless Chrome / Playwright 以 DPR 2 截元素，尺寸与 v2 一致：

- 手机：834 × 1752（417 × 876 @2x），共 18 张
- `pad-ls.png` 1864 × 1304（1194 × 834 @0.78 @2x）
- `pad-pt.png` 1036 × 1484（834 × 1194 @0.62 @2x）
- `board.png` 1560 宽（DPR 1，整页）

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu \
  --hide-scrollbars --force-device-scale-factor=2 --window-size=417,876 \
  --screenshot=shots/wterm.png 'file:///…/index.html?solo=wterm&still=1'
```

## 已知差距

- 桌面取景框是静态的，不可拖动；只表达选区与出口。
- iPad 两向是静态展板，不可点；检查器只画「文件」，`对话` 分段为静态。
- ACP 无终端会话的「全屏对话」没有单独出屏；T7 的行统一回到 T2 + T4。
- 单屏的主题开关走 `?mode=day`；展板用右上按钮。
- 终端、文件、会话内容均为示例数据。
