---
schema: qin-doc/1
kind: plan
id: workbench-layout-plan
title: 开放 Se Manager 工作台的左右停靠、排序、隐藏与拖放布局
date: 2026-11-04
status: draft
source: user-request-2026-11-04-workbench-layout
lang: zh-CN
scope: project
---

# 开放 Se Manager 工作台的左右停靠、排序、隐藏与拖放布局

## 目标

### Objective：交付可恢复、可持久化的工作台布局

将“固定活动栏 + 固定左右侧栏”升级为注册式工作台：用户能将项目、SSH、文件浏览、CLI 会话和插件视图移到左侧、右侧或底部，调整容器/视图顺序、隐藏、折叠、调整尺寸，并一键恢复默认布局。活动栏、标题动作、项目/SSH 工具栏和状态栏也提供各自适用的排序与显隐能力。

成功标准：菜单、拖放、纯键盘和设置页调用同一布局操作；重载后结果一致；既有导航、快捷键、PTY、插件权限与生命周期保持有效；设置与插件管理永远可达。不是把所有页面或按钮任意拖到任意像素位置。

### Evidence：已核实的架构与约束

以下路径均相对 `/Users/qs/project/me/termul`；取证为当前文件读取，未启动应用。知识搜索没有找到可用的 governing knowledge；未将 staged knowledge 或已有 draft 当作事实。

| 区域 | 已核实入口与当前行为 |
|---|---|
| 外壳、左右面板 | `src/renderer/layouts/WorkspaceLayout.tsx:2817–3018` 固定挂载 ActivityRail、ProjectSidebar/ConversationSidebar、PaneRenderer 和右栏；`:2940–3013` 右栏包含 CliSessionPanel、FileExplorer、SSHFileExplorer。手机分支在 `:2760–2814`。 |
| 活动栏与分组 | `components/ActivityRail.tsx:215–239` 读取 navigation-store 并追加活跃插件 sidebar 入口；`:890–938` 折叠按钮、键盘与菜单；`:1016–1026` 折叠保留当前入口；`:1067–1114` 插件入口不参与持久化排序。路径前缀为 `src/renderer/`。 |
| rail 契约与持久化 | `src/shared/types/navigation.types.ts:22–69,108–152` 定义 schemaVersion、revision、entries 和 divider.collapsed；`src/renderer/stores/navigation-store.ts:19,36–50,92–100` 使用 `settings/activity-rail`、revision fence 和 toggleDivider；`hooks/use-navigation-layout.ts:13–64,70–86` 读、归一化、订阅保存。 |
| 项目与 SSH | `src/renderer/components/ProjectSidebar.tsx:1091–1139` 是排序/新建分组/导入/新项目工具栏；`:2156–2159,2283–2302` 将 SSH 作为内部可调整高度区，初值 140、范围 48–400；`components/ssh/SSHPanel.tsx:74–108` 是凭据显隐、导入、新建。 |
| 标题栏与截图差异 | `src/renderer/components/TitleBar.tsx:31–66` 是 Windows/Linux 标题栏；macOS 实现在 `layouts/WorkspaceLayout.tsx:240–277`。历史/右栏按钮位于此处，但终端/地球按钮实际在 `components/workspace/WorkspaceTabBar.tsx:1189–1250`，不是 OS 标题栏。 |
| 状态栏 | `src/renderer/components/StatusBar.tsx:74–135` 固定左右块和插件区；`:136–228` 包含终端恢复、RemoteAccessPopover、退出码、更新提示、Bell、设置。`WorkspaceLayout.tsx:2446` 挂在中央内容内，SSHWorkspace 分支不显示。截图中的屏幕图标来自 `RemoteAccessPopover.tsx:202`，不凭图标新增第二个“屏幕功能”。 |
| 尺寸/设置 | `hooks/use-persisted-panel-size.ts:28–73` 用品牌前缀 localStorage；`WorkspaceLayout.tsx:2845–2848,2932–2935` 左栏 240/180–420，右栏 256/220–560；`:426–432` CLI 高度键后缀 `cli-session-rail-height`，260/140–720。`hooks/use-app-settings.ts:37–43,78–110` 对五种面板显隐串行写入 `settings/app`。 |
| 持久化文件 | `src/renderer/lib/tauri-persistence-api.ts:4–6,30–32,76–85`：通用文件 `termul-data.json`、外层 `{_version:1,data}`、500ms 合并写；`lib/persistence-api.ts:15–17` 选择桌面/web；web 文件路径见 `src-tauri/src/web/mod.rs:463–472`：配置的 store_file 或服务账户状态目录下 `store.json`。 |
| 插件 | `src/shared/types/plugin.types.ts:53–63,539–558`：location=sidebar/panel，uiTree/web，最多 8 views/2 sidebar；`features/plugins/PluginViewPage.tsx:8–48` 两者当前均是中央路由；`PluginViewPanel.tsx:31–47,56–120` 有 generation/epoch 校验；`PluginStatusItems.tsx:20–26,43–60,63–98` 有作用域/TTL/lease 校验。 |
| 快捷键与终端 | `stores/keyboard-shortcuts-store.ts:25,67–106` 快捷键按稳定 ID 而非位置；`ActivityRail.tsx:296–360` 已有 Alt+方向键与 pointer 拖动；`hooks/use-terminal-resize-v2.ts:19,73–87,153–186` 有 8ms fit 合并、40px 下限、隐藏跳过。 |

补充证据：`.git/logs/HEAD:230` 将 `109b51cb` 标为 `feat(rail): collapsible divider groups and a permanent Plugins entry`。已核实当前实现，不声称读过该提交完整 diff。`AGENTS.md:14,23–30` 要求持久化失败日志、shared 契约、桌面/shared-live/se-server/browser parity、双 renderer root 一致、保留 PTY。

### 参照产品与取舍

| 来源 | 采用的原则 |
|---|---|
| [VS Code Custom Layout](https://code.visualstudio.com/docs/configure/custom-layout) | Primary Side Bar 与 Secondary Side Bar 是相对两侧；视图/容器可以拖到侧栏或 Panel；提供 Move View、隐藏、Reset View Locations，用户选择跨会话保留。 |
| [VS Code Tree View API](https://code.visualstudio.com/api/extension-guides/tree-view#view-container)、[Contribution Points](https://code.visualstudio.com/api/references/contribution-points#contributes.viewsContainers) | 区分 view container 与 view；`viewsContainers` 定义容器、`views` 定义成员、`menus.view/title` 定义上下文动作；贡献默认 visible/collapsed/hidden 不覆盖之后的用户设置。 |
| [Obsidian Tabs](https://help.obsidian.md/tabs) | 标签属于组；拖放展示合法 drop zone，支持组内重排和跨组移动；不是所有标签都能放在任意区域。只借鉴交互，不引入其弹出窗口/任意分屏模型。 |

假设与限制：截图可能落后于当前工作树；以已读代码为迁移基准。当前只配置一个桌面 main 窗口，没有独立窗口布局档案。工期是实施估算，不是测试结果。

### Requirements：需求到结果

| ID | Requirement / source | Planned outcome | Acceptance evidence |
|---|---|---|---|
| R1 | 用户希望左右切换、更多可定制区域 | 视图停靠三槽；活动栏两侧；chrome 按能力矩阵移动/排序/隐藏 | L3/L4 DOM 位置断言，项目和 SSH 可分开左右显示 |
| R2 | 截图的 rail、项目/SSH 头部、标题按钮、底部按钮/状态项 | 每个现有入口有稳定 ID；上下文动作保留上下文；不把截图图标误作新功能 | L3/L4 各区域注册项与现有动作回调一一对应 |
| R3 | typed/versioned persisted LayoutState、尺寸、恢复、迁移、作用域 | v1 契约、单一保存入口、兼容旧键、全局布局与窗口几何分离 | L1 round-trip/迁移/故障/双实例测试 |
| R4 | VS Code 类交互、无鼠标可用、中英文 | 相同 reducer 驱动菜单、DnD、设置；焦点/live-region；完整双语 | L4/L5 pointer 与 keyboard 结果一致、Escape 零写入 |
| R5 | 插件 sidebar/panel/status 参加布局且不可失联 | 插件适配器、稳定身份、用户覆盖优先、禁用保留位置、安全恢复入口 | L2 生命周期/TTL/路由测试，L4 实际停靠测试 |
| R6 | 只做计划、分包与 Vitest、终端/性能风险 | 五个互斥写入范围；不改源码/不运行 app/不操作 git | 本文任务与验收；实施命令均标为待执行 |

## 非目标

### Scope

1. 不重写中央 workspace pane tree，不把活跃终端、编辑器、浏览器标签改成全新 docking 系统；不新增窗口、任意浮动窗口、布局云同步或用户脚本执行能力。现有终端列表的 `terminalListPanelVisible` 保持原机制。
2. 不改变 rail 路由页的内容语义：终端看板、Git、历史、会话、快捷终端、Skills、AI channels、MCP、插件管理、定时任务仍打开现有页面/标签；自定义的是入口位置，不强塞完整页面进狭窄侧栏。
3. 不把项目/SSH 的上下文按钮任意搬到全局区域；其头部内可重排/隐藏，隐藏动作保留在不可隐藏的“更多”菜单。状态项只能在状态栏左右互换。
4. 不修改插件 manifest location 枚举、权限、限额、沙箱、宿主协议。布局配置不保存插件运行态、SSH 凭据、终端输出、回调或 generation。
5. 本版全局布局属于当前持久化 store/用户，不提供按项目或独立窗口的完整布局档案；尺寸与激活状态是窗口本地。手机仍采用 MobileChatShell，提供设置/恢复入口但不强行显示桌面三栏或桌面拖放。

## 任务

### Design：统一契约与身份

新增 `src/shared/types/workbench-layout.types.ts`；下列类型是拟新增接口，不是现存符号：

```ts
type DockSlot = 'primary' | 'secondary' | 'bottom' | 'editor'
type ChromeSlot =
  | 'rail.left' | 'rail.right' | 'rail.left.utility' | 'rail.right.utility'
  | 'title.actions' | 'status.left' | 'status.right'
  | 'header.projects' | 'header.ssh' | 'pane.actions'
type LayoutId = string
interface ContainerPlacement {
  slot: DockSlot; order: number; visible: boolean; collapsed: boolean
}
interface ViewPlacement {
  containerId: LayoutId; order: number; visible: boolean; collapsed: boolean
}
interface ChromePlacement {
  slot: ChromeSlot; order: number; visible: boolean
  kind: 'action' | 'divider'; collapsed: boolean; align?: 'left' | 'right'
}
interface LayoutSizes {
  primary: number; secondary: number; bottom: number
  views: Record<LayoutId, number>
}
interface LayoutState {
  schemaVersion: 1; revision: number
  primarySide: 'left' | 'right'
  pluginDefaultMode: 'legacy-editor' | 'manifest'
  containers: Record<LayoutId, ContainerPlacement>
  views: Record<LayoutId, ViewPlacement>
  items: Record<LayoutId, ChromePlacement>
  sizes: LayoutSizes
}
```

`editor` 只供插件兼容中央路由，不获得中央终端树所有权。`primary`/`secondary` 是逻辑槽，物理左右由 primarySide 决定；底部位于中央 pane 下、StatusBar 上。rail.left/right 是物理槽；“交换主副侧栏”同时交换两侧 rail/utility placement，避免入口与容器错位。

注册信息与持久化数据分离：renderer registry 保存 `id/kind/titleKey/defaultPlacement/allowedSlots/canHide/available/render/activate/beforeMove`；JSON 仅保存上述 placement/尺寸。稳定 ID 使用 `builtin:<name>`，插件用 `plugin:<encodeURIComponent(pluginId)>:view:<encodeURIComponent(viewId)>`、`:status:<id>`、`:launcher:<viewId>`。状态项 ID 不含 generation。

新增纯操作 `normalizeLayout(value: unknown): LayoutState`、`reduceLayout(state, command, registry): LayoutResult`；store 提供 `dispatch(command): Promise<LayoutResult>`、`resetLayout(): Promise<void>`、`retrySave(): Promise<void>`。命令固定为 moveContainer、moveView、moveItem、setVisible、setCollapsed、setPrimarySide、resize、reset。所有 UI 禁止直接改 JSON 或直接写 persistence。

归一化：每实体单一 placement；order 为非负整数且同槽排序后压成连续序号；禁止重复 ID、循环/悬空 container 引用、NaN/Infinity、非法槽位和 credential 字段；总实体上限 4096、单 ID 长度上限 256。未知 builtin 丢弃并补齐缺失 builtin；合法但未安装的 plugin ID 保留为休眠覆盖，不渲染。尺寸夹取有效范围，能力约束在 reducer 再校验一次。

### 默认放置、能力与恢复入口

| 实体 | 默认位置及允许操作 |
|---|---|
| context-navigation、SSH | 默认同在 primary 的 builtin:project-context 容器；context-navigation 内按现有路由显示 ProjectSidebar 或 ConversationSidebar。SSH 抽为独立 view，可移出容器，二者可分别停靠 primary/secondary/bottom。 |
| CLI 会话、文件浏览、SSH 文件浏览 | 默认 secondary 的 builtin:inspect 容器，按现有顺序/availability；可独立停靠三槽。SSH 文件视图仍受连接/活动 profile 条件约束，移动不创建连接。 |
| 现有 rail 路由入口、快捷键、主题 | 初始位置继承现有 rail；路由动作允许两侧 rail 与 title.actions；utility 允许两侧 utility/rail/title.actions。移动的是入口，回调/路由不变。 |
| 项目 sort/new-group/import/new-project；SSH credentials/import/new | 仅在自己的 header 槽内排序、隐藏；隐藏项进入“更多”，不可跨到另一个项目/SSH 上下文。 |
| 标题/标签动作及状态 | sidebar/CLI/explorer toggle 保留默认标题位置；terminal-menu/new-browser 默认 pane.actions，可搬到 title.actions，后者操作 activePane、无适用 pane 时禁用。状态内置项与插件项仅允许 status.left/right。 |

设置和插件管理 `builtin:preferences`、`builtin:plugins` 的 `canHide=false`，可移动但不能隐藏；即使所属分组折叠也保持显示。品牌、原生交通灯、最小化/最大化/关闭、拖动窗口保留区不属于可定制项。终端恢复与更新安全提示不允许被用户隐藏；动态未满足条件不等于用户隐藏。每个局部工具栏保留不可隐藏的“更多”，含显示隐藏项和恢复。

默认底部为空且自动收起；有可见容器才展开。三槽都用可折叠纵向 view stack，底部容器之间用 tabs；不引入无限嵌套分屏。拖到空槽时创建 `container:<viewId>` 单视图容器，拖到既有容器时只移动 view；空的自动容器删除，builtin 容器保留身份。容器排序与 view 排序各自独立。

### 持久化、迁移与失败处理

新全局键为 `settings/workbench-layout`，经现有 persistenceApi 写入；不新建 Rust service 或改变外层 `_version`。LayoutState.sizes 是全局初始尺寸：primary=240、secondary=256、bottom=240；primary 范围 180–420、secondary 220–560、bottom 120–600；SSH 高度 140/48–400、CLI 高度 260/140–720。渲染时进一步限制在可用视口内，不将临时窄窗口夹取值写回全局默认。

全局 placement/visibility/collapse 是用户偏好；运行时激活容器与焦点不保存。用户 resize 写窗口覆盖：桌面 main 用品牌前缀 localStorage 的 `workbench-layout-window/main`；浏览器用同后缀 sessionStorage，重载保留、关标签后丢弃，避免远程浏览器改宿主尺寸。对象为 `{schemaVersion:1,sizes:LayoutSizes}`。本版其他桌面窗口尚不存在，不宣称实现多窗口档案。

1. 初始化先读新键；仅 KEY_NOT_FOUND/空值进入迁移，读错误或未知未来 schema 不得作为“缺失”。迁移完成前禁用布局提交，不采用旧 hook 中“本地 revision>0 就盖掉磁盘”的捷径。
2. 一次读取 `settings/activity-rail`、`settings/app` 的 sidebarVisible/fileExplorerVisible/sshPanelVisible/cliSessionPanelVisible、`settings/context-bar`。原 rail entry 顺序、sep-N、三个内置 divider 和 collapsed 完整转换；未出现在 entries 的 utility 保持底部。terminalListPanelVisible 不迁移。
3. 旧品牌宽度 `sidebar-width`、`file-explorer-width`、`cli-session-rail-height` 和 `brandedStorageKey('ssh-panel-height','termul-ssh-panel-height')` 仅迁到当前窗口尺寸覆盖；旧全局设置迁入 LayoutState。插件使用 `pluginDefaultMode='legacy-editor'` 保持既有中央页行为，不等待插件上线才完成迁移。
4. 新安装/显式恢复默认使用 `pluginDefaultMode='manifest'`：sidebar 默认 primary，panel 默认 bottom。具体用户 placement 永远优先；迁移文档中的 legacy-editor 对没有覆盖的插件统一保留旧行为，避免依赖不完整插件快照。写新键成功后才标记 loaded；旧键保留但停止作为运行时写入目标。
5. store 串行提交、合并保存并检查 `IpcResult.success`。失败保留当前预览与 dirty 标记，显示“布局未保存”和重试，不假称成功；后到的旧 write ack 不得清除新 dirty。损坏/未来版本只以内存默认显示并阻止自动覆盖；用户确认恢复时先把原值写到 `settings/workbench-layout/backup`，备份失败则终止恢复。

revision 只防单 renderer 内过期操作，不冒充服务器 CAS。不同客户端不实时广播布局、不在接收他人快照时重排当前窗口；新开会话读取全局最后成功写入。多客户端同时编辑可能 last-writer-wins，设置页明确“全局布局；其他已打开窗口重新加载后应用”。布局协同编辑不在本版范围。

现有导航/面板/context-bar stores 保留公开 API，但在新布局加载后成为投影/委托层；旧 app/context-bar hydration 不能再覆盖新布局。旧快捷键 toggle 调用仍操作对应语义 view，而不是“目前在右边的第一个东西”。恢复只重置布局，不重置项目、SSH、插件 enablement、主题或快捷键。

### UX、可访问性与性能

上下文菜单提供合法目标、上移/下移、显示/隐藏、折叠/展开、恢复本项、恢复布局；不合法目标不显示，受保护项显示禁用原因。设置页列出已隐藏和暂不可用插件项，目标下拉框、显隐开关、前移/后移按钮使用同一 reducer。

拖放只从 rail 图标、容器/view 标题的专用 handle 或“编辑布局”模式中的动作 handle 开始。沿用 pointer 方案和 6px 激活阈值，不新增 DnD 库；禁止从项目行排序区、终端内容、输入框、滚动条或 OS drag-region 发起。拖动期间仅更新临时 overlay/插入线，不写 store；pointerup 成功后一次提交，Escape/pointercancel/window blur/失效贡献取消且零写入。跨 iframe 过程中使用临时透明 shield，结束/卸载一律清理。

保留 rail 的 Alt+Up/Down；焦点在布局 handle 时 Space 开始、方向键选位置、Enter 提交、Escape 取消。Shift+F10/ContextMenu 打开同一个菜单。跨槽提供“移动到…”菜单/设置下拉替代，不新增全局抢占终端的方向键。完成后焦点跟随稳定 ID，隐藏后移到邻项或固定“更多”；aria-live 宣告结果。

| key（shell.layout.*） | 中文 | English |
|---|---|---|
| customize / moveLeft / moveRight | 自定义布局 / 移到左侧 / 移到右侧 | Customize Layout / Move to Left Side / Move to Right Side |
| moveBottom / moveTo / moveEditor | 移到底部面板 / 移动到… / 在主区打开 | Move to Bottom Panel / Move To… / Open in Main Area |
| hide / show / moveUp / moveDown | 隐藏 / 显示 / 上移 / 下移 | Hide / Show / Move Up / Move Down |
| collapse / expand / more | 折叠 / 展开 / 更多 | Collapse / Expand / More |
| resetItem / resetLayout | 恢复此项默认位置 / 恢复默认布局 | Reset Item Location / Reset Layout |
| resetConfirm | 恢复布局位置、显隐、折叠和尺寸？项目与会话不会删除。 | Reset positions, visibility, collapse and sizes? Projects and sessions will not be deleted. |
| protected | 此入口不可隐藏 | This entry cannot be hidden |
| moved | 已将 {{item}} 移到 {{target}}，第 {{position}} 项 | Moved {{item}} to {{target}}, position {{position}} |
| unsaved / retry | 布局未保存 / 重试保存 | Layout not saved / Retry Save |
| pluginReload | 移动将重新加载此插件视图，未保存内容可能丢失。继续？ | Moving will reload this plugin view. Unsaved content may be lost. Continue? |
| globalScope | 全局布局；其他已打开窗口重新加载后应用 | Global layout; other open windows apply it after reload |
| unavailable / canceled | 当前不可用 / 已取消移动 | Currently unavailable / Move canceled |

终端中央根节点及 PaneRenderer key 不因布局 revision 改变；只让 ResizeObserver 处理几何变化，保留 8ms/40px/隐藏跳过。不通过卸载/重建终端解决 resize。只订阅相关 placement slice；pointermove 不触发整个工作台状态更新，尺寸写入在 resize 结束提交一次。拒绝引入整套 docking 框架：其树模型会与现有 PaneDndProvider/PTY 生命周期竞争。

### 插件参与方式

`usePluginWorkbenchContributions()` 从 plugins-store 与 host-ui-store 提供注册描述符；renderer registry 仅负责布局，不取得插件运行时所有权。sidebar/panel 继续作为默认位置提示；用户覆盖不被重装、enable/disable 或新 generation 重置。禁用/卸载保留休眠 placement，设置页可清理，正常工作台不显示死入口。

PluginViewPage 保留原 URL。若 placement=editor，继续当前页面；若已停靠，路由桥接 reveal 对应容器并 replace 到 `/`，待目标已渲染再聚焦，不创建第二份 PluginViewPanel。PluginViewPanel 和 PluginWebView 的安全/会话校验不改。

uiTree 跨容器可以重新挂载并从 host snapshot 恢复；web iframe 跨父容器可能重新加载，不能把 React key/portal 当作保证。registry.beforeMove 对 web 跨容器操作先弹出上述确认；取消不提交，确认后重新校验 generation 再移动。同容器排序使用稳定 key，不能为了拖动复制 iframe。布局不承诺保存插件内部未提交表单。

状态布局覆盖只保存 slot/order/userVisible；最终可见性必须同时满足 userVisible、host visible、TTL、activeProject scope、active+enabled 和 generation。用户“显示”不能复活过期或停用状态项，点击仍执行原 lease 二次检查。新项按 host order/pluginId/id 稳定插入已排序覆盖之后；更新文本不改变用户顺序。插件区有最大宽度/溢出菜单，不挤掉恢复和更新安全提示。

### Execution Plan：五个互斥文件范围包

批准后默认由 `general-executor` 执行；不使用 `.task/TASK-*.json`。下列新增路径均为明确计划创建，并非声称已存在。每包拥有列出的生产文件及其同目录同名测试；其他包只读不改。共享契约由 L1 先冻结，后续新增需求回到 L1 owner，不跨范围顺手改。依赖 DAG：`L1 → L2 → L3 → {L4, L5}`；最终验收等待 L4/L5 都结束。

#### L1 — 契约、单一 store、迁移与兼容投影（2–3 人日）

- **ID / Outcome**：L1；新旧数据能无损读取并通过一个布局状态机写入，现有 API 不产生双写。
- **Files / symbols**：新增 `src/shared/types/workbench-layout.types.ts`、`src/renderer/stores/workbench-layout-store.ts`、`src/renderer/lib/workbench/{registry,reducer,migration,persistence}.ts`、`src/renderer/hooks/use-workbench-layout.ts`；修改 `stores/navigation-store.ts`、`hooks/use-navigation-layout.ts`、`hooks/use-app-settings.ts`、`hooks/use-context-bar-settings.ts`、`stores/{sidebar-store,ssh-panel-store,cli-session-panel-store,file-explorer-store,context-bar-settings-store}.ts`。本包不改 UI、旧 navigation.types 契约或终端列表 store。
- **Read first**：上述被改文件；`shared/types/navigation.types.ts`、`shared/types/contract-guards.ts`、`lib/{persistence-api,tauri-persistence-api,web-persistence-api,log-api}.ts`、`hooks/use-persisted-panel-size.ts`、`types/settings.ts`。
- **Changes**：实现本文 LayoutState/命令/默认/上限/新键；保留 settings/activity-rail 只读迁移；旧 rail/m面板/context-bar API 委托新 reducer；初始化 Promise 去重且 loaded 前不写；未知未来 schema 不覆盖；窗口尺寸覆盖独立；故障使用 log-api 写持久化日志且不记录 JSON 内容/凭据。
- **Dependencies / parallelism**：无依赖；是其他包的契约前置。
- **Acceptance criteria**：迁移 fixture 的 rail 顺序、sep-N、collapsed 和四类 panel visibility 完全保留；迁移运行两次结果等价且只首次写新键；模拟 READ_ERROR 不写，WRITE_ERROR 显示 dirty；迟到 ack 不清掉新 dirty；旧 terminalListPanelVisible 不变；未知版本字节保持不变。
- **Verification**：`bun run test src/shared/types/workbench-layout.types.test.ts src/renderer/stores/workbench-layout-store.test.ts src/renderer/lib/workbench src/renderer/hooks/use-workbench-layout.test.ts src/renderer/stores/navigation-store.test.ts src/renderer/hooks/use-navigation-layout.test.ts src/renderer/hooks/use-app-settings.test.ts src/renderer/hooks/use-context-bar-settings.test.ts`。新测试含两种 persistence facade mock、localStorage/sessionStorage 隔离、两个 store 实例无自动互相重排。

#### L2 — 插件贡献进入统一 registry（2–3 人日）

- **ID / Outcome**：L2；插件 views/launchers/status 在新模型中有安全稳定身份，旧路由仍可用。
- **Files / symbols**：新增 `src/renderer/features/plugins/workbench/{contributions,adapter,route-bridge}.ts`；修改 `features/plugins/PluginViewPage.tsx`、`PluginStatusItems.tsx`；其余 plugin runtime/host/manifest 文件只读。
- **Read first**：L1 registry/store；`src/shared/types/{plugin,plugin-host}.types.ts`、`stores/plugins-store.ts`、`features/plugins/{host-ui-store,PluginViewPanel,PluginViewPage,PluginStatusItems}.tsx/ts` 的实际对应文件、`features/plugins/web/PluginWebView.tsx`、`app/portable-router.tsx`。
- **Changes**：导出 `usePluginWorkbenchContributions()` 与按 ID 渲染单个状态项的 `PluginStatusItem`；旧 `PluginStatusItems` 保留兼容 wrapper。实现 encoded stable IDs、legacy-editor/manifest 默认策略、休眠覆盖、route bridge、web beforeMove 确认请求协议。保留完整 lease/TTL/generation 检查。
- **Dependencies / parallelism**：依赖 L1；为 L3/L4 提供已存在且可编译的插槽适配器。
- **Acceptance criteria**：同插件重启 generation 改变但 placement 不变；disable/uninstall 不删覆盖；re-enable 恢复；过期状态被设置成 visible 仍不渲染/执行；旧 URL 在 editor 原样显示，在 docked 仅 reveal 一份；web 移动取消零写入。
- **Verification**：`bun run test src/renderer/features/plugins/workbench src/renderer/features/plugins/PluginViewPage.test.tsx src/renderer/features/plugins/PluginStatusItems.test.tsx src/renderer/features/plugins/PluginViewPanel.test.tsx`；新增缺少的同名测试文件。用 fake timers 和 generation/bootId 变化断言安全边界。

#### L3 — 可定制动作条、标题栏与状态栏（2–3 人日）

- **ID / Outcome**：L3；现有动作保持语义，布局由 registry 决定，提供 L4 可复用的菜单/动作框架。
- **Files / symbols**：新增 `src/renderer/components/workbench/chrome/{WorkbenchActionStrip,LayoutContextMenu,TitlebarActions}.tsx` 和 `builtin-actions.ts`；修改 `components/{StatusBar,TitleBar,TitlebarPanelToggles,ContextBarSettingsPopover}.tsx`、`components/workspace/WorkspaceTabBar.tsx`；本包独占 `locales/{zh-CN,en}/shell.json` 中完整 layout 字典。
- **Read first**：L1/L2 接口；上述组件；`components/TitleBarShortcutsPopover.tsx`、`RemoteAccessPopover.tsx`、`components/ui/context-menu.tsx`；WorkspaceLayout 的 macOS 标题条只读。
- **Changes**：实现本文 chrome slot 能力矩阵和稳定 ID；`WorkbenchActionStrip({slot,context,actions})` 负责排序、显隐、“更多”和 data-layout-id/slot 标记；TitlebarActions 用同一内容供两个 OS 外壳使用。终端/浏览器在 pane.actions 保持原 callback，放到标题时使用 registry 的 activePane action context。StatusBar 分左右 slot 混合内置/插件项，保留动态 availability、受保护项和溢出。
- **Dependencies / parallelism**：依赖 L2；不修改 WorkspaceLayout/ProjectSidebar，留给 L4 接入；不修改 AppPreferences，留给 L5。
- **Acceptance criteria**：**[UI-observable]** 状态项通过菜单换侧、隐藏后不在条上但仍可从配置恢复；隐藏项目动作后“更多”仍可触发原 callback；标题 action 无可用 pane 时 disabled 而不是创建错误终端；OS 窗口控制和 no-drag 区域保留；所有本文中英文 key 均存在且参数一致。
- **Verification**：`bun run test src/renderer/components/workbench/chrome src/renderer/components/StatusBar.test.tsx src/renderer/components/TitleBar.test.tsx src/renderer/components/TitlebarPanelToggles.test.tsx src/renderer/components/ContextBarSettingsPopover.test.tsx src/renderer/components/workspace/WorkspaceTabBar.test.tsx`。缺失测试由本包新建。

#### L4 — 左右/底部视图宿主、项目/SSH 解耦与统一拖放（4–6 人日）

- **ID / Outcome**：L4；工作台实际支持三槽停靠、两侧 rail、容器/view 重排、项目与 SSH 分离，同时不重建中央 PTY。
- **Files / symbols**：新增 `src/renderer/components/workbench/docking/{WorkbenchDock,ViewContainer,ViewFrame,WorkbenchDndProvider}.tsx`、`src/renderer/lib/workbench-dnd.ts`；修改 `layouts/WorkspaceLayout.tsx`、`components/ActivityRail.tsx`、`components/ProjectSidebar.tsx`、`components/ssh/SSHPanel.tsx`、`components/workspace/ResizableRail.tsx`。新增 `src/renderer/__tests__/workbench-layout.integration.test.tsx`。现有 pane DnD 与 terminal resize 实现只读。
- **Read first**：L1–L3 全部接口；被改文件；`components/workspace/{PaneDndProvider,PanelResizeHandle}.tsx`、`lib/rail-drop.ts`、`hooks/use-terminal-resize-v2.ts`、`components/terminal/ConnectedTerminal.test.tsx`、`components/mobile/MobileChatShell.tsx`。
- **Changes**：在共同 WorkspaceLayout 初始化 layout/插件 bridge，使桌面和浏览器共用；将 SSHResizableSection 从 ProjectSidebar 内部提升为独立 view；右侧三个 view 从固定 JSX 提升为 descriptors；把项目/SSH header 接入 L3 action strip；用 TitlebarActions 替代内联 macOS 按钮。ResizableRail 增加互斥的受控 width/onWidthChange 模式，新宿主不再自行写旧尺寸键。StatusBar 移到桌面统一底部、在 SSH 模式也可提供恢复/设置入口。
- **Changes（拖放）**：provider 通过 data-layout-id/slot 和专用 handle 统一处理 L3 chrome、rail、view、container；计算合法 target，overlay 只保存在 provider，drop 调 dispatch；保留项目条目/中央 pane 拖动隔离。加入底部 resizer、两侧窄屏 overflow 和可访问的展开按钮。保存视图位置不会改变 PaneRenderer key。
- **Dependencies / parallelism**：依赖 L3；与 L5 文件互斥，可并行。
- **Acceptance criteria**：**[UI-observable]** SSH 放右侧、项目仍左侧，CLI 放底部、插件放任一侧可同时显示；同 ID 只有一个 view 实例；容器/视图折叠与隐藏有区别；protected rail 项在分组折叠后可达；100 次 pointermove 对 persistence 调用数为 0、成功 drop 为 1 次逻辑提交、Escape/cancel 为 0；移动侧栏期间 mocked terminal create/kill 调用增量均为 0。
- **Verification**：`bun run test src/renderer/components/workbench/docking src/renderer/lib/workbench-dnd.test.ts src/renderer/__tests__/workbench-layout.integration.test.tsx src/renderer/components/ActivityRail.test.tsx src/renderer/components/ProjectSidebar.test.tsx src/renderer/components/ssh/SSHPanel.test.tsx src/renderer/components/workspace/ResizableRail.test.tsx src/renderer/hooks/use-terminal-resize-v2.test.ts src/renderer/components/terminal/ConnectedTerminal.test.tsx`。覆盖 480/1024/1440 宽度矩形、RTL 不作新承诺、macOS no-drag、手机保持原 shell。

#### L5 — 设置页、命令面板与键盘恢复入口（1.5–2 人日）

- **ID / Outcome**：L5；不用拖动或鼠标也能完成同样配置，隐藏后有明确恢复路径。
- **Files / symbols**：新增 `src/renderer/components/settings/WorkbenchLayoutSettings.tsx`；修改 `pages/AppPreferences.tsx`、`components/CommandPalette.tsx`、`locales/{zh-CN,en}/settings.json`；新增 `src/renderer/__tests__/workbench-layout-accessibility.test.tsx`。keyboard-shortcuts-store 和快捷键配置不改。
- **Read first**：L1/L3 registry、菜单和字典；`AppPreferences.tsx:141–168,401–430`、`components/settings/SettingsLayout.tsx`、`components/CommandPalette.tsx`、`stores/keyboard-shortcuts-store.ts`、`lib/keybinding-schemes.ts`。
- **Changes**：增加类别 id=`workbench-layout`、labelKey=`categories.workbenchLayout`（工作台布局/Workbench Layout），搜索项覆盖布局/侧栏/状态栏/隐藏。设置页按实体展示目标 select、显隐、排序、折叠、尺寸默认值与恢复；列出 dormant 插件并解释“当前不可用”。命令面板新增稳定 ID `workbench.layout.customize`、`workbench.layout.reset`、`workbench.layout.moveView`，不绑定新全局默认组合键；现有设置/插件命令不按 rail visibility 过滤。
- **Dependencies / parallelism**：依赖 L3；与 L4 并行。共享窗口说明使用 shell.layout.globalScope，不重复改 shell.json。
- **Acceptance criteria**：**[UI-observable]** 仅 Tab/方向键/Enter 可将 SSH 移右、状态项移左、显示被隐藏项并恢复；菜单与设置对相同命令产生完全相同 LayoutState；protected 项开关 disabled 且说明可读；焦点不会丢到 body；既有终端快捷键不被新增全局 listener 截获；从空 rail 仍可进入 preferences/plugins。
- **Verification**：`bun run test src/renderer/components/settings/WorkbenchLayoutSettings.test.tsx src/renderer/pages/AppPreferences.test.tsx src/renderer/components/CommandPalette.test.tsx src/renderer/__tests__/workbench-layout-accessibility.test.tsx src/renderer/stores/keyboard-shortcuts-store.test.ts`。新建缺少的测试；mock dispatch 验证 UI 不直接写 persistence。

## 验收

### Validation：实施后的命令与观察证据

工作目录固定为 `/Users/qs/project/me/termul`。以下均为待执行命令；本次规划没有运行测试或桌面程序。

| 检查 | 命令或可观察结果 | 覆盖 |
|---|---|---|
| 分包功能 | 执行每包 Verification；新增测试应实际被 Vitest 发现，不能用 passWithNoTests 掩盖缺失 | R1–R5 |
| 类型与格式 | `bun run typecheck`；`bun run check`；预期退出码 0，既有非相关失败单独记录，不归因或顺手修复 | 契约/范围 |
| 完整回归 | `bun run test`；`bun run test:conversation-first`；所有新功能与既有导航/会话/PTY 测试通过 | R1–R5 |
| 跨入口 parity | `bun run test src/renderer/__tests__/renderer-root-parity.test.tsx src/renderer/lib/__tests__/parity-checklist.test.ts`；Tauri 与 web facade 均加载新布局；mobile 不出现桌面 DnD handle | R3–R5 |
| 保存/恢复场景 | 迁移旧 rail 含自定义分隔与 collapsed，移动 SSH/插件、修改状态栏、重载、模拟写失败、重试、恢复；断言旧数据保留且会话/插件启用状态不变 | R1–R5 |

jsdom 不能证明原生窗口拖动、真实 iframe 重载或 xterm GPU 渲染。实施完成后另行授权桌面冒烟：macOS/Windows/Linux 窗口控制、左右边栏 resize 时持续输出的 PTY、SSH 连线不中断、web iframe 移动确认、共享远程和独立 se-server 浏览器布局；当前“不得运行桌面应用”约束继续有效，不能在此计划阶段擅自执行。

### Risks and Recovery

| 风险 | 防护与恢复 |
|---|---|
| 双源写入或迁移覆盖用户配置 | L1 兼容 API 统一委托；未知 schema 只读保护；旧键不删除；用户恢复前保存原值 backup。回退旧版本仍能读旧键，但不会获得升级后的新布局修改。 |
| xterm 尺寸/PTY 与隐藏生命周期 | 中央 pane tree 身份不变；保留 ResizeObserver 的 debounce/零尺寸保护；mock 检查 create/kill 零增量；真实渲染留待授权冒烟。 |
| iframe/插件状态丢失或过期命令 | web 跨容器移动先确认重载风险；不克隆 iframe；generation/TTL/lease 保持二次校验；取消保留原位置，失败只显示 unavailable，不自动重复执行动作。 |
| 多客户端覆盖与性能 | 明确 last-writer-wins、非协同布局；窗口尺寸隔离；hover 零持久化、drop 一次提交、slice 订阅；故障保留 dirty 并提供重试。 |
| 被隐藏入口、键盘冲突、窄窗口 | settings/plugins/恢复安全项不可隐藏；局部更多不可隐藏；不抢全局终端方向键；小窗口临时 overflow 不篡改全局布局；恢复仅影响布局。 |

### Open Decisions

无阻塞的用户决策：本草案已固定“受约束的工作台定制、全局位置偏好 + 窗口本地尺寸、保留中央终端树、旧插件先兼容中央页、web 跨容器移动确认重载”五项边界。它们是待批准的产品方案，不是假定用户已经确认；若批准时改变任何一项，应先修订对应契约/迁移/验收，不让执行者临场决定。

预计总实施量 **11.5–17 人日**；可在 L3 后并行 L4/L5，另预留跨平台原生冒烟时间。下一步：将本稿保存到 `.qin-code/docs/plans/workbench-layout-plan.md` 并审核上述五项边界，再批准 L1。
