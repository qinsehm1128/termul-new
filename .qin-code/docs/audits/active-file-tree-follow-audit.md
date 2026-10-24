---
schema: qin-doc/1
kind: audit
id: active-file-tree-follow
title: 活动文件与文件树联动验收
date: 2026-10-24
status: draft
source: user:2026-10-24T11:00:56+08:00; agent://active-file-tree-discovery; agent://active-file-tree-tests; agent://active-file-mobile-parity; agent://active-file-tree-review
lang: zh-CN
---

## 结论

已实现当前文件与文件树的单向联动：打开文件、切换编辑器标签时标记活动文件，桌面文件树自动展开祖先并以nearest滚动定位。活动标记与操作选择集独立，不更改多选、删除目标或编辑焦点。最终225项相关测试、完整类型检查和独立审查通过。

此前本地构建/安装任务仍按用户10:43要求暂停。本任务不构建原生应用、不关闭应用、不安装或发布。

## 发现

- 根因是编辑器的`useActiveFilePath`镜像已跟随工作区活动标签变化，但文件树仅订阅手动`selectedPaths`，两者没有联动。
- `FileTreeNode`增加活动文件的`aria-current`、轻背景和左侧标记，手动多选样式与操作目标保持独立。`MobileFileExplorer`在当前目录列表中使用相同活动标记，不改变手机目录导航或持久化位置。
- 新`useRevealActiveFile`只在文件树可见、非搜索、非创建/重命名时运行。不切换根，不关闭手动展开目录，不抢焦点。支持多根目录、Windows路径分隔符，目录加载未完成时等待，变化/隐藏/卸载时取消旧定位。
- 真实store测试复现两种边界：A→B→A项目快速切换时旧目录请求清除新loading，同路径根重置时旧attempted保留。分别用目录请求的root-set identity保护和hook的rootScope identity重启修复，均有红绿证据。
- desktop、shared-live、standalone server浏览器界面共用portable router/WorkspaceLayout，因此共用桌面树修复。手机列表单独补齐。SSH/SFTP文件浏览使用独立编辑/预览状态，不接入本地活动文件路径。

## 证据

- 最终命令：`bun run test -- src/renderer/components/file-explorer src/renderer/components/mobile/MobileFileExplorer.test.tsx src/renderer/stores/file-explorer-store.test.ts src/renderer/layouts/WorkspaceLayout.close-persistence.test.tsx src/renderer/__tests__/renderer-root-parity.test.tsx`。
- 结果：10个文件、225项测试通过，无失败。覆盖高亮切换、空活动文件、目录排除、选择集不变、路径边界、多根目录、异步展开、隐藏/搜索跳过、快速切换、旧请求失败和同路径重置。
- `bun run typecheck`的node/web/test三套检查通过，变更10个源/测试文件Biome无错误，`git diff --check`通过。原有a11y warnings未扩大修复范围，部分既有布局测试输出模拟WebSocket关闭提示但测试通过。
- 独立`active-file-tree-review`在两项竞态修复与新增测试后给出approve。
- 浏览器隔离夹具使用真实FileTreeNode、真实editor/explorer store与真实reveal hook，仅文件系统/拖动等外部边界替换为受控数据。实际点击嵌套文件后逐层加载、高亮；滚动树后切换App.tsx确认唯一活动标记、目标在可视区、焦点留在原按钮；切换到终端后活动标记清除，手动README选择保持不变。
- 浏览器证据：`/Users/qs/.qin-code/scratch/active-file-tree-ui-FYAtJizW/active-nested-file.png`、`browser-verification.json`、`final-tests.log`。测试Space和预览服务已关闭，未连接运行中的Se后端或PTY。

## 残留风险

- 尚未在重新打包的Tauri应用或真实Windows环境中做端到端交互测试，不宣称当前已安装程序含此变化。
- 手机端只对当前目录已显示文件标记，不因活动文件变化强行改变用户浏览目录。
- 隐藏、搜索和inline编辑时不自动展开，恢复文件树正常状态后再跟随当前文件。
