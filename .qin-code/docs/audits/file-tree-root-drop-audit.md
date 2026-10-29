---
schema: qin-doc/1
kind: audit
id: file-tree-root-drop
title: 文件树根目录拖放修复验收
date: 2026-10-30
status: draft
source: user:2026-10-29T23:52:16+08:00; commit:0d48d768; agent://file-tree-root-drop-review; agent://root-drop-browser-harness; agent://root-drop-touch-resolver
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_dolphin_1792723909440_951d256685611548","last_updated_by_session":"session_dolphin_1792723909440_951d256685611548","session_ids":["session_dolphin_1792723909440_951d256685611548"],"todo_refs":[{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-drop-discover"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-drop-implement"},{"session_id":"session_dolphin_1792723909440_951d256685611548","id":"root-drop-verify"}],"goal_refs":[]}
---

## 结论

已修复“文件树内只能拖进子文件夹，不能拖到根目录”的 UI 接线缺口，源码提交为 `0d48d768`。

- 单根视图新增始终可见的根目录行，并允许拖到树的空白区。
- 根目标使用当前显示的 `rootPath`，因此 worktree 或外部目录视图不会错误落到侧栏记录的另一个项目目录。
- 多根视图可拖到对应项目根标题，不能在目标不明确的全局空白处猜测一个根。
- 内部文件拖拽仍走移动，Finder 外部文件拖入仍走复制；文件行、编辑控件、搜索/错误视图以及不安全/无效目标不会误回退为根移动。
- 鼠标和长按触屏共用目标解析规则。

469 项相关测试、完整 TypeScript 检查及独立审查通过。隔离浏览器中，单根根目录行、空白区和文件行误落防护均通过实际拖拽验证。多根第二个根的实际浏览器拖拽没有完成，下文如实记录其工具限制；对应目标选择已有自动化回归通过。

未自动发布、打包、安装或重启 Se，没有操作真实用户文件或系统剪贴板。

## 发现

### 根因与修复

原内部拖拽只由 `FileTreeNode` 的目录行接收。单根视图不渲染根行，树空白区只处理外部 Finder 文件，多根分组标题也没有拖放处理器，因此用户无法在树内选择根目录作为目标。

修复仅涉及四个 renderer 文件：

- `FileExplorer.tsx`：新增根目录可见行和 `data-tree-root-path` 目标标记；根处理器复用 store 拖拽状态、`isRejectedMove`、`moveEntries` 与 `importDroppedFiles`，隔离事件冒泡、清除高亮，提供错误提示和不含路径的失败日志。
- `use-tree-long-press-drag.ts`：增加 `root` 目标类型。目录/文件条目优先于包围它们的根标记；文件和可编辑控件不能退回根目标，保留既有项目行识别。
- 对应两个测试文件：增加根空白、可见根行、worktree 路径、多根精确目标、外部复制、无效/歧义目标、失败返回/rejection 与触屏解析回归。

根目录行放在滚动区外，树内容铺满时仍有明确落点。新增区域使用带名称的 section 语义，不把没有点击操作的区域伪装成按钮。

没有修改 store 的移动业务、Tauri/Rust 文件服务、web 授权策略或 SSH 文件树。桌面与浏览器共享的 FileExplorer 获得同样接线，已有不同 transport 的权限限制保持；独立 MobileFileExplorer 不因此新增此前没有的拖放能力。

### 校验过程中修正

新增表格测试最初把数组参数当成整组路径，已修正夹具。根移动失败文案最初引用了错误 namespace，类型检查发现后改为复用 `projects:fileContext.moveFailed`。没有删除测试或放宽断言。

工具服务重载中断了独立审查和浏览器夹具执行，恢复时读取已有产物而不是重做代码。最终单个任务结果均持久可读；迟到的任务图 incarnation 通知不作为测试失败或重新运行应用的理由。

## 证据

### 自动化与审查

- `root-drop-final-regression.log`：11 个文件、469 个用例通过，覆盖文件树、目录节点、长按目标、文件 store、项目侧栏、两个 renderer root 和 parity 台账。
- `typecheck-final.log`：node、web、test 三套 TypeScript 检查通过。源码提交钩子再次通过类型检查。
- `biome-final.log` 与提交输出：格式检查无错误，保留 6 个原有按钮/分隔条语义告警，未为本次根目标新增告警。
- 独立 `file-tree-root-drop-review`：当前四文件 diff APPROVED，0 findings，`git diff --check` 通过；没有把父会话测试结果伪称为 reviewer 独立重跑。
- `git status --short`：源码提交后没有 tracked 工作区变更，原有用户未跟踪文档、知识、锁文件及 pycache 保留。

证据根目录：`/Users/qs/.qin-code/scratch/file-tree-root-drop-c8t2p9sy`。测试和浏览器夹具均在隔离环境，不执行真实文件移动。

### 浏览器行为验证

`root-drop-browser-harness` TaskSpace 38 使用真实 `FileExplorer`、`FileTreeNode` 及实际的 `resolveLongPressDropTarget`/`useTreeLongPressDrag`。父会话再次读取了夹具 hook 桥接，确认它直接重新导出仓库实现，而不是复制一套 root 解析逻辑。文件系统、native API、持久化及外部服务被替换为内存夹具。

| 场景 | 结果 |
| --- | --- |
| `/project/src/nested.txt` 拖到可见根目录行 | 一次 `moveEntries`，目标 `/project`，没有选择变化 |
| 同一嵌套文件拖到单根空白区 | 新增一次移动调用，目标 `/project`，没有重复冒泡调用或选择变化 |
| 拖到普通文件 `/project/root.txt` | 移动调用数量不变，没有误落到根 |
| 多根 DOM 目标 | `/project`、`/other` 只在各自标题上有根标记，全局空白没有标记 |
| 固定高度布局 | 1728×900 viewport 中，文件面板 256×700；单根根行及空白区可到达 |
| 多根第二个根的实际拖拽 | **未完成**。工具要求起点和终点同时可见；滚动到目标后源位于视口外，不把该项当作已通过 |

所有浏览器移动仅记录在 scratch 内存 store 中，不是用户文件的真实移动。实际操作记录保存在任务结果 `agent://root-drop-browser-harness`，不以模块 HTTP 200 代替交互验收。

验收结束后，父会话确认 Space 38 只有 Agent 创建的 `http://127.0.0.1:41783/` 测试页并关闭它，取消隔离 Vite task `439748n67y`。旧 task `770553o0wb` 已因重载终止。证据文件保留，没有删除或关闭用户页面/应用。

## 残留风险

- 多根第二个根的实际浏览器手势未完成，触屏端也没有进行真实设备手势验证。对应目标解析与多根移动参数已有自动化测试，不能混同为全部设备端到端验证。
- 浏览器使用实际组件与内存文件服务，未对安装中的 Tauri/WebKit 或真实文件系统做移动实验。
- 原有移动服务的同名预检查与底层 rename 之间仍可能有竞争窗口，本次未扩展为事务性文件系统改造，不承诺并发下绝对无覆盖。
- 修改已提交源码，尚未构建安装到用户当前运行的应用，因此需要后续明确的构建/发布/安装操作才会进入运行版本。
- 当前修复无已知代码阻塞；上述未运行的检查均保留为明确限制，不宣称通过。
