---
schema: qin-doc/1
kind: research
id: file-preview-path-research
title: 文件链接和预览问题的实现参考
date: 2026-11-05
status: draft
source: 2026-11-05 用户问题及本轮第一方网络检索
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_crocodile_1792734636572_27d03ccc6cf693c1","last_updated_by_session":"session_crocodile_1792734636572_27d03ccc6cf693c1","session_ids":["session_crocodile_1792734636572_27d03ccc6cf693c1"],"todo_refs":[{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"investigate"}],"goal_refs":[{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"goal-2d7024f9-fbf1-4486-9437-bc902ffbe878"}]}
---

## 问题
Markdown 预览目录持续闪烁，终端文件链接匹配或定位失败，以及 Finder 可打开的图片在应用中无法预览。

## 来源
- VS Code TerminalLinkResolver：https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/contrib/terminalContrib/links/browser/terminalLinkResolver.ts 。去除行列后缀，以终端 cwd 和主机 OS 做路径规范化，验证 stat，隔离本地/远程缓存。
- VS Code TerminalSearchLinkOpener：https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/contrib/terminalContrib/links/browser/terminalLinkOpeners.ts 。先精确 cwd 路径，再工作区搜索，只有唯一精确匹配才自动打开，歧义交给 quick pick。
- xterm WebLinkProvider：https://raw.githubusercontent.com/xtermjs/xterm.js/master/addons/addon-web-links/src/WebLinkProvider.ts 。先拼接 wrapped buffer 行，再通过 getCell/getChars/getWidth 把字符串索引映射为终端坐标。不能将 UTF-16 字符串索引视为屏幕列号。
- React useEffect：https://react.dev/reference/react/useEffect 。不稳定函数或对象依赖可触发反复 effect，state 更新和依赖变化能形成循环。视觉测量必要时使用 layout effect，但不可用它掩盖真实循环。
- Tauri convertFileSrc：https://v2.tauri.app/reference/javascript/api/namespacecore/#convertfilesrc 。asset protocol 必须启用，文件必须处于 scope 中，对应协议 origin 必须被 CSP 放行。
- MDN 图像格式指南：https://developer.mozilla.org/en-US/docs/Web/Media/Guides/Formats/Image_types 。WebView/浏览器支持的解码格式与 Finder 系统解码能力并不等价。

## 结论
修复方向须遵循：链接识别与坐标映射分离，终端/文档上下文优先于项目根目录，搜索降级只接受唯一明确结果，不在任意目录猜测。预览刷新应保持已有内容和稳定的 DOM/订阅。图片必须通过应用授权的跨端文件读取通道并保留真实 MIME、加载状态与错误。

## 未证实
目前仅检索外部实现。本仓库具体根因待三条只读代码调查与回归测试确认。用户未提供失败图片格式，不能声称能通过截图确定格式不支持。
