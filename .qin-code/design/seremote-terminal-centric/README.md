# SeRemote · 以终端为中心

可点的静态原型，对应提案 `docs/proposals/seremote-terminal-centric-ia-proposal.md` 的 T1–T8 与 iPad 两向。
一个自包含的 `index.html`（CSS/JS 内联，沿用 v2 的 token、组件与图标），加 `shots/` 截图。

## 打开

| 目的 | 地址 |
|---|---|
| 展板（全部屏 + iPad） | `index.html` |
| 单屏 | `index.html?solo=<id>&still=1` |
| 日间 | 追加 `&mode=day`；或点展板右上「日间」 |
| iPad 横 / 竖 / 整屏预览 | `?solo=pad&o=ls` / `?solo=pad&o=pt` / `?solo=pad&o=preview` |

`solo` id：`home` `terminals` `wterm` `wterm-keyboard` `wterm-nohook` `files` `files-preview` `files-immersive` `files-md` `attach` `attach-sent` `chat` `desktop` `more` `more-confirm` `conversations` `tools` `settings` `git` `tasks` `search` `screen` `drawer`。iPad 另有 `pad-preview`（`?solo=pad-preview`，等同 `?solo=pad&o=preview`）。

## v2 评审改动

**文件 = 列表 + 整屏预览**

- T3 只留面包屑与文件列表，列表吃满 sheet；内联预览卡与底部动作行都删掉。
- 点文件开整屏 cover（`files-preview` 代码、`files-md` Markdown），盖住整台手机，终端也在下面。
- 顶栏：左 ✕，中文件名 + 「342 行」，右 ⋯。⋯ 菜单：插入路径 / 发给 Agent / 复制 / 分享。
- 底部只一颗主操作「→| 插入路径」：写进终端并回到 T2 + toast。
- 点内容切 chrome（`files-immersive`，无顶栏无按钮）；✕ 或下滑关闭，grabber 是下滑提示。内容点按不关闭，避免和滚动、选字冲突。
- iPad 从检查器点文件，同一个 cover 盖住整台 iPad：`pad-preview`。

**收起占位**

- T2（`wterm` / `wterm-nohook`）没有命令行，也没有常驻按键行，终端吃到 home indicator。
- 终端右下角一颗玻璃胶囊，两枚图标按钮：`[＋]` 附件、`[⌨]` 键盘（都有 aria-label）。
- `[⌨]` → `wterm-keyboard`：系统键盘 mock，按键行作为键盘配件贴在上面（`Esc Tab Ctrl ↑ ↓ ← → ⌘V` + 收起），敲键直接进 PTY，没有独立输入框。键盘弹起时胶囊隐藏，`[＋]` 挪到配件行左端。
- iPad 同步删掉按键行与输入：胶囊在终端窗格右下；另加「硬件键盘」提示，信息图标给出一个 tooltip。

**本地文件**

- `[＋]` → `attach`：照片 / 拍照 / 文件，外加目标行「发送到 → zsh — Termul ▾」。
- 目标规则与桌面一致：对话就绪发 Agent，否则写进终端。
- 选「文件」→ `attach-sent`：toast「已上传 report.pdf · 路径已写入终端」，提示符里出现 `~/.se-manager/attachments/2026-11-10/report.pdf`。
- 上传走主机已有的附件暂存路由，文件落在主机的暂存目录，不进项目。

## 与 v2 的差异

**移除**

- 三模式开关（会话 / 终端 / 文件 分段）：终端不再被模式切走，文件与对话改为终端上的面板。
- Dashboard 式首页（主机 → 会话 → 工具）：连接后直接落在终端。
- 终端内的会话选择器（带侧栏的会话列表）：hook 会话只保留一条细带。
- 独立的 console 屏与 `/console` 入口。
- 桌面入口由 6 个减到 2 个：标题栏的文件、桌面。
- 常驻按键行与命令行输入框（v2 评审）。

**新增**

- 落地页的「继续」hero、项目分组、快捷终端、新建终端。
- 终端标题栏：`⌄` 同项目终端切换、文件、桌面、⋯。
- hook ribbon（`Claude Code · 工作中 · 1 个许可 · 对话 ›`）；无 hook 时整条消失。
- 文件：列表 sheet、整屏 Quick Look 式预览、沉浸态、⋯ 菜单。
- 媒体与文件入口：照片 / 拍照 / iOS 文件选择器，附件暂存到主机再回写路径。
- 桌面 sheet：取景框、目标终端、发送选区 / 整屏。
- 更多动作表 + 关闭确认（`关闭后该 shell 会结束`）。
- iPad：250 侧栏 + 中栏 T2 + 320 检查器；竖屏 76 轨道 + 右侧滑入检查器；整屏预览一屏。

## 屏幕

| # | id | 屏 | 说明 |
|---|---|---|---|
| 01 | `home` | 主机 | 未连接主机列表；连接后去终端 |
| 02 | `terminals` | 终端 | T1 落地：继续、项目分组、快捷终端、新建终端 |
| 03 | `wterm` | 工作台 | T2：标题栏 + 标签 + PTY + ribbon + 全高终端 + 右下玻璃胶囊 |
| 04 | `wterm-keyboard` | 工作台 | 键盘弹起：配件行在键盘上，输入直通 PTY |
| 05 | `wterm-nohook` | 工作台 | 同一屏，无 hook ribbon |
| 06 | `files` | 文件 | T3：只有面包屑 + 列表，列表吃满 sheet |
| 07 | `files-preview` | 预览 | 整屏代码 cover：✕ / 文件名 + 行数 / ⋯，底部插入路径 |
| 08 | `files-immersive` | 预览 | chrome 全隐藏，点内容回到 07 |
| 09 | `files-md` | 预览 | Markdown 走同一套整屏 cover |
| 10 | `attach` | 附件 | `[＋]`：照片 / 拍照 / 文件 + 目标行 |
| 11 | `attach-sent` | 附件 | 上传后 toast + 提示符里的暂存路径 |
| 12 | `chat` | 对话 | T4：大 sheet over T2，气泡、读取 chip、许可卡、composer |
| 13 | `desktop` | 桌面 | T5：CSS 桌面、取景框、目标终端、发送选区 / 整屏 |
| 14 | `more` | 更多 | T6：七个动作 + 红色关闭终端 |
| 15 | `more-confirm` | 更多 | 关闭确认，只写后果 |
| 16 | `conversations` | 会话 | T7：待你处理 + 代理会话列表 |
| 17 | `tools` | 工具 | T8：更改 / 定时任务 / 搜索 / 屏幕文本 + 最近项目 + 设置 |
| 18–21 | `git` `tasks` `search` `screen` | — | 沿用 v2，返回目标是 `wterm` |
| 22 | `settings` | 设置 | 主题、通知、字号、凭据 |
| 23 | `drawer` | 抽屉 | 终端 / 会话 / 工具 + 主机 + 设置 |
| 24 | `pad` `o=ls` | iPad 横 | 1194 × 834：250 + T2（无按键行）+ 320 检查器（纯列表） |
| 25 | `pad` `o=pt` | iPad 竖 | 834 × 1194：76 轨道 + T2 + 滑入检查器 |
| 26 | `pad` `o=preview` | iPad 预览 | 整屏 cover 盖住整台 iPad |

## 已连的流程

- T1 行 / hero → T2
- T2 `[⌨]` → `wterm-keyboard`；键盘「收起」→ T2
- T2 `[＋]` → `attach`；「文件」→ `attach-sent`（toast + 路径写进提示符）
- T2 文件图标 → `files`；点文件 → `files-preview` / `files-md`；「插入路径」→ T2 + toast
- `files-preview` ⋯ → 菜单（插入路径 / 发给 Agent / 复制 / 分享）
- `files-preview` 点内容 → `files-immersive`；再点 → `files-preview`
- T2 ribbon → T4；T4 ✕ → T2；T7 行 → T2 + T4
- T2 桌面图标 → T5；「发送选区」/「整屏」→ T2 + toast
- T2 ⋯ → T6；「关闭终端」→ 确认；确认后 → T1 + toast；「对话」→ T4
- T2 标题 `⌄` → 终端切换；标签页切换标题
- 底栏 终端 / 会话 / 工具；抽屉 → 三处 + 设置

## 截图

在 `shots/`，用 headless Chrome 以 DPR 2 截单屏，尺寸与 v2 一致：

- 手机：834 × 1752（417 × 876 @2x），23 张
- `pad-ls.png`、`pad-preview.png` 1864 × 1304（1194 × 834 @0.78 @2x）
- `pad-pt.png` 1036 × 1484（834 × 1194 @0.62 @2x）
- `board.png` 1560 宽（DPR 1，整页 26 格）

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu \
  --hide-scrollbars --force-device-scale-factor=2 --window-size=417,876 \
  --screenshot=shots/wterm.png 'file:///…/index.html?solo=wterm&still=1'
```

## 已知差距

- 下滑关闭只有 grabber 提示，没有真实拖拽手势。
- 键盘是静态 mock，按键不会真的输入；附件入口不接 PHPicker / UIDocumentPicker。
- ⋯ 菜单里的「发给 Agent / 复制 / 分享」只出 toast，不接系统分享面板。
- iPad 展板静态不可点；`pad-preview` 是单独一屏，检查器里点文件的手势由这一屏表达。
- 桌面取景框是静态的，不可拖动。
- 终端、文件、会话内容均为示例数据。
