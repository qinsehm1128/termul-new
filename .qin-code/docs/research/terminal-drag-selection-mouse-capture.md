---
schema: qin-doc/1
kind: research
id: terminal-drag-selection-mouse-capture
title: AI终端拖选间歇失效：鼠标捕获、重绘与状态恢复调查
date: 2026-10-24
status: draft
source: user:2026-10-24T09:22:12+08:00; user:2026-10-24T09:23:05+08:00; agent://terminal-selection-paths; agent://xterm-selection-upstream; agent://agent-tui-selection-upstream
lang: zh-CN
---

## 问题

2026-10-24用户报告：在Se中运行Claude Code与qin-code，最初能鼠标拖选复制，过一段时间无法拖选，但鼠标/键盘其他操作正常。退出AI后普通shell能拖选，重新进入AI后暂时恢复。用户要求联网检索xterm.js等相关issue并多角度分析。

本轮仅调查、读取代码、联网与独立内存实例验证。未改终端行为，未向活PTY发送任何输入或控制码，未退出、重启或安装应用。

## 来源

### 上游一手来源（2026-10-24查阅）

1. [Claude Code fullscreen官方文档](https://code.claude.com/docs/en/fullscreen#keep-native-text-selection)：明确区分应用内部选区与终端原生选区，fullscreen会捕获鼠标，原生copy-on-select停止；文档有`CLAUDE_CODE_DISABLE_MOUSE=1`及功能损失说明。`CLAUDE_CODE_DISABLE_MOUSE_CLICKS=1`仍保留鼠标捕获，原生拖选仍需修饰键。文档是当前版本说明，不证明用户安装版本及设置。
2. [Claude Code #61936](https://github.com/anthropics/claude-code/issues/61936)：2.1.150相对2.1.148的右键/原生拖选报告，维护者归因默认fullscreen并关闭。仅支持相应Claude路径，不能外推qin-code。
3. [xterm.js #4903](https://github.com/xtermjs/xterm.js/issues/4903)：1003与1006组合下，强制选区会在鼠标移动后被清除。作为[#4297](https://github.com/xtermjs/xterm.js/issues/4297)的重复项关闭，不等于明确修复了所有类似症状。1006是SGR鼠标编码，不是独立的拖动跟踪开关。
4. [xterm.js #2975](https://github.com/xtermjs/xterm.js/issues/2975)：新xterm连接到已运行应用时不会自动取得原有鼠标模式，恢复由宿主/序列化负责。支持检查重连和状态恢复，但不是当前“后来拖选失效”的直接复现。
5. [xterm.js #468](https://github.com/xtermjs/xterm.js/issues/468)：输出导致选区消失的历史问题，已在旧版2.8.0对应重构中解决。仅作为“高亮出现后消失”这一独立故障类别，不把旧bug直接归因于当前6.1 beta。
6. [xterm.js #5220](https://github.com/xtermjs/xterm.js/issues/5220)：5.5.0的DOM同步刷新/替换导致后续click事件丢失，6.0.0里程碑修复。涉及事件投递，不是已确认的当前拖选根因。
7. [xterm.js API](https://xtermjs.org/docs/api/terminal/interfaces/iterminaloptions/)、[当前类型定义](https://github.com/xtermjs/xterm.js/blob/master/typings/xterm.d.ts)、[SelectionService源码](https://github.com/xtermjs/xterm.js/blob/master/src/browser/services/SelectionService.ts)：强制原生选区和mouseEventsRequireAlt是受支持的输入路由选项。
8. [XTerm控制序列参考](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)：备用屏幕与鼠标跟踪是不同协议状态。
9. [tmux #3705](https://github.com/tmux/tmux/issues/3705)：alternate screen中拖动/滚轮事件在tmux copy-mode与应用之间的转交。仅在用户确实使用tmux时相关。

没有找到能确认身份的qin-code公共issue或专属禁用鼠标配置。已检索本构建qin-code bundled docs，通用终端兼容说明不能作为本次qin-code运行模式的证据。不能把Claude环境变量套用到qin-code。

### 当前代码证据

- `package.json:172`、`bun.lock`、`node_modules/@xterm/xterm/package.json:4`一致：当前源码/依赖锁为`6.1.0-beta.315`。尚未读取用户故障窗口实际运行包的版本。
- `src/renderer/components/terminal/terminal-config.ts:12-48`设置`rightClickSelectsWord`，未设置`macOptionClickForcesSelection`或`mouseEventsRequireAlt`。
- 该版本`SelectionService.ts:437-446`：若mouseEventsRequireAlt启用且鼠标模式active，未按Alt则强制原生选区；否则macOS必须Alt且macOptionClickForcesSelection为true，非macOS走Shift。
- `OptionsService.ts:42-44`：两项选项默认均为false。因此不能泛泛建议本项目macOS用户“Shift拖选即可”，也不能保证Option在现配置下有效。
- `MouseService.ts:380-391`：应用鼠标模式active且mouseEventsRequireAlt未启用时，直接disable selection service；模式关闭则重新enable。
- `src/renderer/utils/terminal-registry.ts:121-155`被动跟踪SET/RESET，不吞协议。`:191-202`恢复当前打开的模式。
- `ConnectedTerminal.tsx:1848-1856`模式恢复在transcript回放前。`se-pty/src/modes.rs:1-11`说明宿主preamble是“最早保留日志字节之前”的状态，不是误用过去状态的直接证明。
- Ghostty宿主observer接收PTY输出副本，现有接线不负责renderer鼠标选区，也不向PTY回写鼠标控制序列。

### 独立内存实例验证

用当前真实`@xterm/xterm`创建独立Terminal，所有控制序列仅写入此内存实例，非用户终端。结果：

| 输入阶段 | mouseTrackingMode | buffer |
|---|---|---|
| 初始 | none | normal |
| 仅1049h备用屏幕 | none | alternate |
| 再1002h+1006h | drag | alternate |
| 仅1049l退出备用屏幕 | drag | normal |
| 再1002l+1006l | none | normal |

证明备用屏幕状态不等于鼠标跟踪状态，两者不能互相替代诊断。

## 结论

### 优先级排序

1. **最高优先：应用启用鼠标上报，加上Se当前macOS缺少强制原生选区的配置。**与“AI内失败、普通shell恢复”高度一致。协议机制和配置均已确认，但未确认失效瞬间确实发生mode变化。
2. **条件性：AI内部选区或输出重绘问题。**TUI可自行绘制高亮并复制，不等于xterm原生选区。如果初期的拖选其实由应用处理，后来应用内部复制/坐标命中/流式重绘出错，也能产生相同描述。要区分完全无高亮和高亮闪现后消失。
3. **条件性：Se重连、项目切换或恢复模式的时序问题。**确有模式恢复路径，只有与故障同时出现replay/remount/attach才提高优先级。不能仅凭“恢复函数只发ON、renderer源码搜不到OFF字面量”认定缺陷，关闭序列可来自原始PTY输出，而新xterm本来默认OFF。
4. **环境性：tmux/SSH的额外输入路由或剪贴板层。**需先确认是否存在。复制失败与无法建立选区不是同一层故障。
5. **较低优先：Ghostty观察器或普遍鼠标硬件问题。**现有被动observer边界与正常shell行为不支持优先归因于它们。未做活现场验证，不能宣称彻底排除。

### 推荐后续动作

- 在故障前后只读观察实际`mouseTrackingMode`、buffer类型、选区是否建立/消失、最近是否发生terminal restore/replay。协议日志只记录状态切换，不记录用户终端正文。
- 最小兼容改进候选：为macOS显式启用`macOptionClickForcesSelection`，保留应用鼠标功能，Option拖动强制选字。
- 更符合“默认拖选”的候选：当前xterm已支持`mouseEventsRequireAlt:true`。普通拖动走终端选区，Alt/Option点击拖动才交给应用，滚轮不受此选项影响。属于交互策略变化，不应在本轮调查中擅自启用。
- Claude专项对照可在用户下一次自行开启的新测试会话中用官方`CLAUDE_CODE_DISABLE_MOUSE=1`，代价是Claude鼠标点击及内部滚轮功能损失。不修改全局shell配置，不终止当前Agent，不给qin-code套该变量。
- 不建议第一步打印全量鼠标RESET或执行`reset`，这会改变现场并可能打断TUI交互，却无法证明原因。

## 未证实

- 故障窗口实际鼠标模式、Claude/qin-code版本、是否tmux/SSH、触发时是否切项目/重连，均未采集。
- 没有证据证明xterm经过固定时间会自行启用mouse tracking。
- 尚未证明哪一方发出了触发序列，也不能把同一症状归因于同一个上游issue。
- 未对产品进行修复或重新打包安装。本报告的机制复现不等于复现了用户那一次时序故障。
