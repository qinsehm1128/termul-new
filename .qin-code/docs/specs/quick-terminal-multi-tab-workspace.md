---
schema: qin-doc/1
kind: spec
id: quick-terminal-multi-tab-workspace
title: 快捷终端多终端与文件标签工作区契约
date: 2026-10-26
status: draft
source: user:2026-10-26T15:14:24+08:00; session_search:current:turn1795; agent://quick-workspace-discovery
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-contract"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-host"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-ui"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"quick-tabs-verify"}],"goal_refs":[]}
---

## 契约

用户2026-10-26 15:14明确批准同步实现：同一快捷条目内多个终端，以及文件预览与终端统一标签栏。不是在左侧创建更多快捷条目来替代多个终端。

### 宿主与传输

- `QuickTerminalRecord.schemaVersion=2`增加`terminalIds: string[]`，保留`terminalId`作为首个终端的兼容别名，而非全局活动标签。v1从原terminalId生成列表后兼容读取，后续修改按v2持久化。不可读或未来版本的记录保留原文件。
- `open(id, cols, rows, terminalId?)`复用已有活终端，可指定属于该条目的终端。`createTerminal(id,cols,rows)`在同一cwd新增终端，不能替换其他终端。
- `closeTerminal(id,terminalId)`只关闭归属于该条目的指定终端。原`close(id)`关闭条目全部终端但保留条目，`remove(id)`关闭全部后移除记录。
- 新Tauri命令为`quick_terminal_create_terminal`、`quick_terminal_close_terminal`，均使用`payload`。原open只增加可选terminalId。
- HTTP沿用现有flat路径：POST `/quick-terminals/create-terminal`与`/quick-terminals/close-terminal`，body分别为`{id,cols,rows}`和`{id,terminalId}`；原`/quick-terminals/open`路径不变。
- 新建返回既有`QuickTerminalOpened`结构，含record、terminalId、claim、spawned。claim仍走现有一次性交接，禁止持久化claim。所有终端写入record的conversationId，projectId保持空，避免被普通项目错误接管。

### 界面

- 每个快捷条目独立终端/文件混合标签栏，顶部加号在当前条目中创建终端。左侧新增菜单仍负责新条目/新目录。
- 打开文件创建或激活文件标签，点击终端标签即可返回，关闭文件回到合理的现有标签。不能静默丢弃未保存编辑。
- 新终端保留首次网格测量后才spawn的既有行为，避免启动尺寸不符造成提示符重复。
- 标签布局按quickTerminalId隔离持久化，重入时与宿主terminalIds对账，复用活PTY，不把普通项目全局workspace root用作快捷工作区状态。
- 窄屏要有可用的工作区宽度、横向可滚动标签和返回列表入口，桌面与Web使用同一功能。

## 不变量

1. 切标签、预览文件、切快捷条目或导航页面不得结束用户PTY。关闭单个终端不得影响其他终端。
2. 宿主按条目串行化并校验终端归属，外来terminalId不得被接管或终止。新建后持久化失败必须清理刚创建的PTY。
3. v2终端ID必须非空且无重复，primary别名若存在必须等于首项，Opened终端必须在record列表内。
4. 普通项目与快捷工作区的活动编辑器/终端互不覆盖。父布局在quick路由暂停项目pane镜像，回项目立即恢复同步。
5. backend共享服务、Tauri、shared-live/standalone Web facade一起实现，不靠renderer私有孤儿PTY作为最终方案。
6. 本轮不发布、不安装、不重启运行中的Se，不向用户活PTY注入测试输入。只使用隔离测试PTY/夹具。

### 文件所有权

- `quick-tabs-host`：相关Rust quick-terminal记录、持久化、服务、core IPC、Tauri命令、Web路由及对应Rust测试。
- `quick-tabs-ui`：quick-terminal UI/状态store/新工作区组件/相关测试及quickTerminal语言资源，不修改API facade或shared契约。
- 父：shared quick-terminal types、Tauri/Web facades及API parity测试、WorkspaceLayout活动状态隔离hook和测试、跨端checklist、最终集成与独立review。

## 验收

- v1记录迁移后原活终端可复用，future/corrupt记录不被覆盖。
- 同条目创建2个不同终端，切换/预览文件不重建活实例，单关保留其他终端，关闭全部及删除收敛全部归属。
- 文件预览可返回终端，未保存编辑安全，页面重入/刷新可恢复标签并接管活PTY。
- 桌面/Web两个新接口及指定open payload、返回值、错误码等价。窄屏交互可用，普通项目布局与状态不受干扰。
- 运行focused Vitest、node/web/test类型检查、Rust服务与适用传输测试、静态检查。使用独立浏览器夹具验证交互并独立审查完整diff后提交。
- 当前父会话已验证：旧Tauri与新契约parity共21项通过；新活动状态隔离hook、文件打开和布局回归19项通过；跨端checklist与根一致性170项通过。最终集成结果仍待两条实现线完成。
