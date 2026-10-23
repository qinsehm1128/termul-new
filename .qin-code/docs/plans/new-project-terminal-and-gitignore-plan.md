---
schema: qin-doc/1
kind: plan
id: new-project-terminal-and-gitignore
title: 新建项目终端引擎与自定义Git忽略规则
date: 2026-10-23
status: draft
source: user:2026-10-23T22:24:27+08:00; session_goal:passed; agent://new-project-host-review
updated: 2026-10-24
lang: zh-CN
---

## 目标

响应用户2026-10-23 22:24 +08:00需求：新建项目直接选择终端屏幕观察引擎，创建Git项目时可编辑默认`.gitignore`内容。

默认保留用户原文`.qin-code/`与`.se-manage/`，并追加当前brand实际工作目录（正式版`.se-manager/`、Canary`.se-manager-canary/`），去重。拼写确认一次等待超时，已告知兼容策略。用户可以修改或清空，非全局设置。

## 非目标

- 不改变默认引擎existing，也不把Ghostty变成终端渲染器。
- 不重建现有PTY，不影响已有活会话。
- 克隆、导入不追加自定义忽略规则。
- Web的终端引擎设置保持宿主只读。原生iOS没有创建入口，本次不新增契约。
- 不改当前仓库的`.gitignore`，不自动发tag、push、部署、安装或停止用户应用。
- 文件更新不是针对同权限恶意宿主进程的沙箱，不承诺阻止其他已具备rename/link权限的进程在操作期间修改inode关系。

## 任务

### 已完成

1. `NewProjectModal`复用`TerminalScreenEngineSetting`，创建及克隆将引擎作为第六参数传入store，默认existing。克隆保留原shell第四参数，第五envVars为空。项目立即保存引擎，沿既有持久化链同步。
2. Git初始化启用且路径检查符合条件时显示可编辑规则。执行顺序：mkdir、git init成功、模板scaffold成功、自定义规则追加成功、创建回调。空内容不写，失败不伪报成功。
3. 共用`GitApi.addIgnoreRules(cwd, content)`，Tauri命令`git_add_ignore_rules`与Web `POST /git/ignore-rules`委托同一Rust服务。Web沿用local-only、路径验证及mutating分类。
4. 模板根`.gitignore`也改为安全合并，不再调用截断写。其他模板文件行为不变。
5. Rust服务保持旧字节前缀与规则顺序，按完整行的末尾块幂等，不做全局逐行去重。兼容CRLF、无末尾LF、裸CR。空白不创建，输入NUL/过量、非普通文件、已有symlink/reparse/hardlink拒绝。协作调用用OS文件锁串行化。Unix使用目录fd+openat、O_NOFOLLOW/O_NONBLOCK，Windows保护目录与文件句柄并检查reparse与链接数。
6. Desktop log与server tracing覆盖开始、成功/no-op、拒绝和失败，仅记录长度、状态及错误分类，不记录正文、cwd或原始错误。
7. 修复独立审查发现的异步shell检测重置表单、旧目录探测影响新路径两处竞态，并补deferred-promise回归。

### 审查及集成

- 模板改动：`project-template-ignore-review`批准。
- UI改动：`new-project-ui-review`提出两处异步问题，产品修复并以真实IpcResult mock回归验证。
- 宿主改动：`new-project-host-review`提出Windows目标替换、并发幂等、缺日志和硬链接问题，全部整改。补目录句柄和换行边界后最终复审批准，保留明确威胁模型限制。
- 最终证据见同目录体系下audit `new-project-terminal-and-gitignore-verification`。

## 验收

2026-10-24 00:35 +08:00机械Goal验收通过：

- 全项目node/web/test TypeScript检查。
- `NewProjectModal*.test.tsx`、project-store、default helper、模板、Git facade、HTTP transport、parity清单。
- 17项Rust共享服务测试，6项新HTTP handler测试，完整web router测试。
- 独立se-server编译、desktop/web两套生产构建、git diff检查。
- 另行通过55项既有持久化/宿主同步/renderer根组件回归及Rust workspace格式检查。
- Windows真实服务源码和测试通过x86_64-pc-windows-msvc交叉类型检查，但没有Windows运行时测试。
- 隔离浏览器挂载真实弹窗，模拟宿主验证Ghostty选择与custom ignore回调、Web只读说明。未对真实项目执行创建。
