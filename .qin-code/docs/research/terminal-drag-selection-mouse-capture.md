---
schema: qin-doc/1
kind: research
id: terminal-drag-selection-mouse-capture
title: AI终端拖选间歇失效：鼠标捕获、重绘与状态恢复调查
date: 2026-10-24
status: draft
source: user:2026-10-24T09:22:12+08:00; user:2026-10-24T09:23:05+08:00; agent://terminal-selection-paths; agent://xterm-selection-upstream; agent://agent-tui-selection-upstream; isolated-repro:2026-10-24T09:36+08:00
updated: 2026-10-24
lang: zh-CN
---

## 问题

2026-10-24用户报告：在Se中运行Claude Code与qin-code，最初能鼠标拖选复制，过一段时间无法拖选，但鼠标/键盘其他操作正常。退出AI后普通shell能拖选，重新进入AI后暂时恢复。用户要求联网检索xterm.js等相关issue并多角度分析。

本轮仅调查、读取代码、联网与独立内存实例验证。未改终端行为，未向活PTY发送任何输入或控制码，未退出、重启或安装应用。

## 来源

### 上游一手来源（2026-10-24查阅）

1. [Claude Code fullscreen官方文档](https://code.claude.com/docs/en/fullscreen#keep-native-text-selection)：明确区分应用内部选区与终端原生选区。fullscreen捕获鼠标后原生copy-on-select停止。`CLAUDE_CODE_DISABLE_MOUSE=1`关闭捕获但损失应用点击和内部滚轮功能；`CLAUDE_CODE_DISABLE_MOUSE_CLICKS=1`仍保留捕获，原生拖选仍需修饰键。当前文档不证明用户实际安装版本/设置。
2. [Claude Code #61936](https://github.com/anthropics/claude-code/issues/61936)：2.1.150相对2.1.148的右键/原生拖选报告，维护者归因默认fullscreen并关闭。仅支持相应Claude路径，不能外推qin-code。
3. [xterm.js #4903](https://github.com/xtermjs/xterm.js/issues/4903)：1003与1006组合下，强制选区会在鼠标移动后清除。作为[#4297](https://github.com/xtermjs/xterm.js/issues/4297)重复项关闭，不等于明确修复了所有类似症状。1006是SGR鼠标编码，不是独立拖动跟踪开关。
4. [xterm.js #2975](https://github.com/xtermjs/xterm.js/issues/2975)：新xterm连接到已运行应用不会自动取得原有鼠标模式，恢复由宿主/序列化负责。支持检查重连状态，不是当前症状的直接复现。
5. [xterm.js #468](https://github.com/xtermjs/xterm.js/issues/468)：输出导致选区消失的历史问题，已在旧版2.8.0对应重构中解决。仅作为“高亮出现后消失”的故障类别，不把旧bug直接归因当前6.1 beta。
6. [xterm.js #5220](https://github.com/xtermjs/xterm.js/issues/5220)：5.5.0的DOM同步刷新/替换造成后续click丢失，6.0.0里程碑修复。涉及事件投递，不是当前拖选根因的证明。
7. [xterm.js API](https://xtermjs.org/docs/api/terminal/interfaces/iterminaloptions/)、[类型定义](https://github.com/xtermjs/xterm.js/blob/master/typings/xterm.d.ts)、[SelectionService](https://github.com/xtermjs/xterm.js/blob/master/src/browser/services/SelectionService.ts)：强制原生选区和mouseEventsRequireAlt是受支持的输入路由选项。
8. [XTerm控制序列参考](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)：备用屏幕与鼠标跟踪是不同状态。
9. [tmux #3705](https://github.com/tmux/tmux/issues/3705)：alternate screen中拖动/滚轮在tmux copy-mode与应用间转交。仅在实际使用tmux时相关。

没有找到能确认身份的qin-code公共issue或专属禁用鼠标配置。检索本构建qin-code bundled docs得到通用终端兼容说明和内部选区状态描述，但不能证明故障窗口当前模式。不可把Claude变量套用到qin-code。

### 当前源码及配置

- `package.json:172`、`bun.lock`、`node_modules/@xterm/xterm/package.json:4`一致锁定`6.1.0-beta.315`。不是已核实用户故障窗口运行包版本。
- `src/renderer/components/terminal/terminal-config.ts:12-48`设置rightClickSelectsWord，未设置macOptionClickForcesSelection或mouseEventsRequireAlt。
- 依赖`SelectionService.ts:437-446`：mouseEventsRequireAlt启用且鼠标模式active时，未按Alt强制原生选区；否则macOS要求Alt且macOptionClickForcesSelection=true，非macOS走Shift。
- `OptionsService.ts:42-44`两项默认false。因此不应泛泛建议本项目macOS用户“Shift拖选即可”，Option现配置也未启用。
- `MouseService.ts:380-391`：应用鼠标模式active且mouseEventsRequireAlt未启用时disable选区服务，模式关闭后enable。
- `terminal-registry.ts:121-155`被动跟踪SET/RESET且返回false，不吞协议；`:191-202`从记录生成模式恢复字节。
- `ConnectedTerminal.tsx:1848-1856`在transcript前恢复模式。`se-pty/src/modes.rs:1-11`明确宿主preamble表示最早保留日志前的模式，不能仅因它表示过去状态就判错。
- Ghostty宿主observer接收PTY输出副本，不负责renderer拖选，未发现其回写鼠标控制序列路径。

### 实验一：备用屏幕与鼠标模式独立

真实@xterm/xterm独立内存Terminal，非用户终端：

| 阶段 | mouseTrackingMode | buffer |
|---|---|---|
| 初始 | none | normal |
| 仅1049h | none | alternate |
| 再1002h+1006h | drag | alternate |
| 仅1049l | drag | normal |
| 再1002l+1006l | none | normal |

证明备用屏幕与鼠标跟踪不能混为一谈。

### 实验二：真实Se追踪器可复现RESET语义偏差

2026-10-24 09:36 +08:00，用Bun直接导入仓库真实`startTrackingModes/getTerminalModes/buildRehydrateSequences`，连接独立xterm实例A，向新独立实例B恢复快照。不是重写模拟逻辑，也没有连接活PTY。

| 阶段 | xterm实际模式 | Se记录 |
|---|---|---|
| A收到1002h | drag | drag |
| A收到1000l | none | drag |
| B收到Se生成的恢复字节 | drag | 来自错误的drag记录 |

直接原因：当前xterm `InputHandler.ts:2236-2240`对9/1000/1002/1003中的任意RESET统一把activeProtocol设NONE。Se `terminal-registry.ts:90-97`仅在RESET编号匹配当前记录的模式时清空，所以1002h之后1000l会分叉。

复现核心（只操作新建内存实例）：

```ts
startTrackingModes('isolated-selection-repro', original)
await write(original, '\x1b[?1002h')
await write(original, '\x1b[?1000l')
// original.modes.mouseTrackingMode === 'none'
// getTerminalModes('isolated-selection-repro').mouseTracking === 'drag'
await write(restored, buildRehydrateSequences(getTerminalModes('isolated-selection-repro')))
// restored.modes.mouseTrackingMode === 'drag'
```

这是已证实的Se模式追踪/恢复缺陷，但未证明用户那次运行确实发出这个序列组合并经过恢复路径。不是已确认的宿主Rust tracker复现，也不能扩展成所有回放都错误。

## 结论

1. **直接机制最高优先：鼠标上报把拖动交给AI，不再由xterm建立原生选区。**机制与macOS缺少强制选区配置已确认，失效瞬间mode仍待现场取证。
2. **Se存在已复现的模式记录/恢复错误。**真实xterm已关闭，但Se记录仍开，恢复可重新启用。若失效紧接项目切换、视图重建或重连，应优先检查。若全程没有恢复/重建，不能用此实验单独解释故障。
3. **应用内部选区或流式重绘问题仍是独立备选。**AI可能自行绘制高亮并复制，初期能拖选不必然是xterm原生选区。要区分完全无高亮、高亮闪现消失、以及有高亮但复制失败。
4. **tmux/SSH与平台输入路由有条件相关。**需先确认存在。复制失败与无法建立选区属于不同层。
5. **Ghostty观察器/鼠标硬件不是优先嫌疑。**被动observer接线及普通shell正常不支持优先归因，但未做活现场验证，不能绝对排除其他集成问题。

不能把“恢复函数只发ON、renderer源码搜不到OFF字面量”本身认定为严重缺陷。RESET可来自原始PTY输出，新xterm默认OFF。应使用实验二的具体语义分叉作为证据。

### 建议处理顺序

- 先修Se已证实的模式追踪语义偏差，增加跨编号RESET及恢复到新实例的回归。对所有相关完整reset/模式切换需按实际xterm行为验证，不能仅靠mock。
- 同时增加只读状态诊断：实际mouseTrackingMode、buffer类型、选区事件及最近restore/replay。只记录状态，不记录用户终端正文。
- 交互策略二选一：最小兼容改进为Mac启用macOptionClickForcesSelection，Option强制选字；更符合默认拖选习惯的是mouseEventsRequireAlt=true，普通拖动选字，Alt/Option才交给AI，滚轮不受该选项影响。两项不是简单同时堆叠，后者优先。
- Claude专项对照可在另开的测试会话用官方CLAUDE_CODE_DISABLE_MOUSE=1，代价是应用鼠标点击/内部滚轮损失。不改全局配置、不结束当前Agent、不外推qin-code。
- 不先向正在使用的终端打印全量RESET或执行reset，避免破坏现场与TUI交互。本轮不擅自实施产品修复。

## 未证实

- 故障窗口实际模式、Claude/qin-code版本、是否tmux/SSH、发生时是否切项目/重连，尚未采集。
- 未证明用户实际输出含实验二的序列组合，也未证明哪个应用触发了它。
- 无证据显示xterm经过固定时间会自行启用鼠标上报。
- 上游issue只是相关机制和问题类别，不能替代当前故障复现。实验二证明仓库有bug，不等于证明它是本次现场根因。
- 本轮未修改产品、打包或安装，只有调查报告更新。
