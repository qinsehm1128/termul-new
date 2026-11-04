---
schema: qin-doc/1
kind: runbook
id: plugin-web-ui-author-runbook
title: Native 插件 Web UI 编写、打包与验证
date: 2026-11-04
status: accepted
scope: project
source: .qin-code/docs/plans/plugin-web-ui-plan.md; .qin-code/scratch/plugin-webui-plan-full.md
lang: zh-CN
---

## 适用

面向为已有 native 插件增加 Web UI 的作者。示例是 `src-tauri/crates/se-plugin-example/ui/` 的 **Activity Studio**：HTML、CSS、经典 JavaScript 静态文件，无前端编译、无 CDN、无新增运行时依赖。native 插件负责计数器、终端/agent 汇总、活动采样与伴随服务；UI 只负责展示和有权限的宿主 API 调用。

本文是操作与验收清单，不是已完成桌面隔离验收的报告。本次实现阶段**禁止启动桌面应用**；普通浏览器、模拟 bridge 与 DOM 测试不能证明真实 WKWebView/WebView2 隔离。桌面清单需要另行授权，不得为完成文档自动启动应用。

元数据约束：需求指定的 ID 为 `plugin-web-ui-author`，但 qin-doc 写入校验要求 ID 与文件名 slug 一致，本路径只能接受 `plugin-web-ui-author-runbook`。本文保留可校验 ID；这一差异必须在交付报告中明确，不伪称已满足原始 ID 要求。

## 步骤

### 1. 资源与信任边界

- `index.html` 使用相对路径 `./app.css`、`./app.js`。JavaScript 是普通 `defer` 经典脚本，不使用 ESM、eval、内联脚本或宿主 imports。
- 入口显式使用 `<script src="../__se__/se-plugin-ui.js"></script>` 在 app.js 前加载宿主保留的 `__se__/se-plugin-ui.js`；从 `ui/index.html` 的 URL 解析到同 session 根目录。该保留路径由宿主提供，不能将自制 shim 打进插件包覆盖。当前协议处理器也会注入 shim，shim 的 `if (window.se) return` 保证重复执行幂等。权威实现是 `src-tauri/src/plugin_ui_protocol.rs` 与 `src/renderer/features/plugins/web/shim/se-plugin-ui.js`。
- 唯一宿主入口是 `window.se`。不得直接调用 Tauri invoke/event、文件系统、shell 或父文档。诊断区只观察 Tauri globals 是否存在，并捕获父文档读取异常，不保留/遍历父文档内容。
- iframe 使用 `sandbox="allow-scripts"`，不得加入 `allow-same-origin`。文档安全 origin 与资源 URL origin 不同；opaque origin 通常序列化为 `null`，不能用 URL origin 证明隔离。
- 不引用外部脚本、字体或图像。事件、配置、笔记均用 `textContent` 展示，SVG 用命名空间 DOM API 构造，不拼接 `innerHTML`。

### 2. Manifest、权限与 context

在已有 native 插件 ID 下添加 web view，示例 view ID 固定为 `activity-studio`，标题为 `Activity Studio`，入口为 `ui/index.html`。保留原有 Counter、Activity 等 native 贡献。具体字段以协议 DTO 与当前示例 manifest 为准。

原生命令 ID 是 `increment`、`reset`、`summary`、`service-ping`，必须声明在 `contributes.commands`。web view 使用 `kind:"web"`、`entry:"index.html"`、`location:"sidebar"`、共享图标白名单中的 `icon:"chart-bar"`，以及能力列表 `allow:["commands","context.read","context.events","storage.read","storage.write","feedback","config.read"]`；其中 commands 是粗粒度 bridge 能力，不是逐命令 ID 的子列表。未声明的 `config.write`、`view.dispatch` 会被拒绝。

`services.requires` 保留 `se.host.context`、`se.host.events`、`se.host.storage`、`se.host.feedback`、`se.host.status`，并增加 `se.host.ui`、`se.host.wake`、`se.host.process`，每项 `version:1`。该系统不把插件 native 代码当作网络安全沙箱；插件可以调用任意本地或远程服务。本例为可重现演示仅调用 loopback，无外部资源。

Context 监听复用已有 manifest 订阅。`se.context.onEvent(id, callback)` 只注册本地 listener，不发起动态 subscribe RPC：

| 订阅 ID | topic |
| --- | --- |
| `project` | `project.activeChanged` |
| `terminal-created` | `terminal.created` |
| `terminal-closed` | `terminal.closed` |
| `terminal-active` | `terminal.activeChanged` |
| `activity` | `agent.session.stateChanged` |

`contributes.processes` 声明 `{id:"activity-service",command:"native/se-plugin-example-service",args:["--port","0"],autostart:true,restart:"never"}`。命令相对归档根目录，页面不自行 spawn。HTTP 服务只监听 `127.0.0.1` 动态端口，提供 GET `/ping`，第一行 stdout 输出 `SE_ACTIVITY_PORT=<port>`，同时记录 stdout/stderr 日志。native 插件在 create 创建唯一 worker（不在普通回调启动线程）；wake 回调通过 `se.host.process/logs` 发现端口，worker 用有界 TCP 超时探测并在回调外请求 wake。正常禁用/升级由宿主管理 stop；Unix 示例另外轮询 getppid，在 Plugin Core 崩溃、服务被重新托管到其他父进程后自行退出，防止孤儿服务。该自保护不能替代宿主对任意第三方进程的崩溃清理。

### 3. Shim API 与生命周期

先 `await window.se.ready`，再使用当前公共 API：

```js
const summary = await window.se.invoke('summary', {});
const offMessage = window.se.onMessage((summary) => { /* 更新 UI */ });
const theme = await window.se.theme.get();
const offTheme = window.se.theme.onChange((theme) => { /* 更新派生显示 */ });
```

- `summary` 是快照，不等待 HTTP。`service-ping` 也立即返回快照，native 后台完成后通过 `onMessage` 推送相同 payload 形状。
- 字段是 `{count,terminalCount,agentTurnCount,activity:[{label,...}],service:{port,ok,pid,uptimeMs,error},wakeCount}`。未连接或缺字段显示未知，不伪造 0。
- `se.context.snapshot({scope:'activeProject'})` 读取权威上下文。时间线至多保留 12 条。项目切换、`context.resync` 或 `se.onResyncRequired` 后重新读取 context 与 summary，不把本地 reducer 当作权威。
- `se.onInvalidate` 普通刷新重取快照；会话撤销/版本变化错误应停用操作并重开 view，不做无界重试。`pagehide` 清理 listeners、定时器与 fetch。
- `se.config.get()` 的 `values` 是有权限的只读配置快照；`se.config.onChange` 后重新读取，不依赖 iframe 重载。
- 通知使用真实 DTO：`se.feedback.notify({requestId,id,title,message,severity:'info'})`。bridge、权限或服务错误应显示给用户，不能吞掉。

### 4. 主题与无障碍

`window.se.theme.get()` 返回 Promise，字段是 `appearance:'light'|'dark'`（不是 `isDark`）。shim 将 `tokens` 投影到根节点；示例使用宿主 HSL 通道 token，如 `hsl(var(--background))`、`--foreground`、`--card`、`--muted-foreground`、`--border`、`--primary`，并提供暗色默认值。字体来自 `--se-font-family`、`--se-font-size`。

检查 320px、760px、1120px 宽度，明暗主题都可读且无横向溢出。保留可见 focus、label、状态 live region、SVG title/desc 与 reduced-motion。没有宿主 shim 的独立预览只展示断连状态与布局，不伪装已连接。

### 5. 笔记 CAS 与冲突

固定使用插件隔离的 profile scope（wire 值为 `profile`，权限名仍为 `storage.plugin`）的键 `activity-studio.notes`，值为 `{text:'...'}`。先 `se.storage.get({requestId,scope:'profile',key})`，保存返回 revision；写入使用 `se.storage.set({requestId,scope:'profile',key,value,expectedRevision})`。每次操作生成新的本地 requestId。

- revision/expectedRevision 是整数。缺失键也返回 revision，新键通常为 0，删除后的 tombstone 可能更大；不能传 null 或假设总是 0。
- `PLUGIN_STORAGE_CONFLICT` 时保留本地草稿并停止保存。用户先复制草稿，再明确选择“Load latest (replace draft)”，人工合并。不得自动取新 revision 重试覆盖。
- 保存期间继续编辑，成功回包只将提交时的草稿标为已存，新编辑仍显示 unsaved。普通失败不清空草稿。
- get 失败后先显式重试加载，再启用编辑与保存。关闭重开必须从宿主存储恢复，而不是只依赖页面内存。

### 6. 服务两条探测链路

**Native ping** 调用 `se.invoke('service-ping', {})`。UI 显示请求已提交，native 后台 HTTP 完成后从 summary 显示 `service.ok/error`。不能把立即返回的旧快照当作新探测成功。native worker 通过 `se.host.wake/request` 请求唤醒，宿主管理的回调 service/method 是 `se.host.wake/wake`（不是 `deliver`）；native 在该回调中处理结果并推送 summary，浏览器不直接调用此后台协议。

**Direct fetch /ping** 从有效整数 port 构造唯一目标 `http://127.0.0.1:<port>/ping`，使用 `mode:'cors'`、`credentials:'omit'`、`redirect:'error'` 和 5 秒中止。不使用 no-cors 掩盖 opaque response，不跟随跳转到外部站点。

opaque-origin iframe 请求通常带 `Origin: null`，服务需响应 `Access-Control-Allow-Origin: *`，不启用 credentials/cookie。无自定义头的 GET 一般不需要预检。CSP connect-src 也必须允许已设计的 loopback 访问，页面不能自行放宽宿主 CSP。native 成功不等于浏览器 CORS 成功，两项独立显示。

### 7. 完整包与安装升级

先按已批准构建流程生成对应 triple 的 native library 和 service 二进制。静态 UI 无需 npm/vite build。归档根目录包含 `package.json`、`manifest.json`、`native/<triple>/<library>`、`native/se-plugin-example-service`、`ui/index.html`、`ui/app.css`、`ui/app.js`。`package.json` 为 `{schemaVersion:1,native:{"<triple>":"native/<triple>/<library>"},ui:"ui",files:[{path,size,sha256}]}`；files 覆盖除 package.json 本身以外的所有归档文件，sha256 为 64 位小写十六进制。manifest 的 web `entry` 相对 ui/，所以值为 `index.html`。

一键构建和验收是 `bash scripts/test-plugins.sh`；其调用 `scripts/test-plugin-web-ui.sh` 构建两个无新增依赖的 Rust bin，生成 `$SE_PLUGIN_TEST_ARTIFACTS/{normal,fault,upgrade}/activity-studio.seplugin`，默认根目录 `/Users/qs/project/me/termul/src-tauri/target/plugin-test-artifacts`。归档安装前验证 inventory/hash 和目标架构；加载后还会比较 JSON manifest 与 ABI manifest，坏 hash 不得加载 native。

打包 CLI（从仓库根目录执行，替换占位符）：

```sh
cargo run --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-example --bin se-plugin-pack --locked -- --manifest <manifest> --target <triple> --native <lib> --ui <ui> --service <bin> --out <archive>
```

`<ui>` 是包含入口 HTML 的静态目录；`<triple>` 与 native/service 架构匹配，例如 `aarch64-apple-darwin`。共享 target 需串行安排，不与其他 cargo 作业并发。打包器应执行路径、扩展名、symlink、大小与完整 manifest 校验，不手动 zip 绕过校验。

另行授权进入桌面验收后，通过宿主插件管理 UI 导入 archive，确认已有权限提示包含服务与 loopback。启用后从 Counter 面板、Plugins sidebar 或 Command Palette 打开 `Activity Studio`。升级应导入新完整包，不原地修改已安装文件；旧 session 必须失效，重开从新 generation 加载，升级前保存笔记。禁用/卸载应撤销 UI 请求并停止服务；单独关 UI 不等于停止 native 模块。

## 失败时

| 症状 | 检查与安全处理 |
| --- | --- |
| Host bridge unavailable / 按钮禁用 | 确认从宿主 contribution 打开，而不是双击 HTML；检查 shim 注入/版本/handshake，不暴露 Tauri globals。 |
| PLUGIN_PERMISSION_DENIED | 检查 manifest 和用户已授予现有权限，不扩大 bridge allowlist。 |
| PLUGIN_STORAGE_CONFLICT | 复制草稿，显式加载最新 revision，人工合并；不得自动 overwrite。 |
| Native ping 成功、Direct fetch 失败 | 分别检查 ACAO、CSP connect-src、有效端口、IPv4 loopback、进程状态。fetch TypeError 本身无法区分 CORS/CSP/网络。 |
| PLUGIN_SERVICE_UNAVAILABLE / 端口缺失 | 检查架构、artifact、启动/握手、端口发现与退出日志；UI 不自行 spawn。 |
| Session 失效 | 关 view 并重开，不复用旧 token/session 或跨 generation 请求。 |
| 无 context 事件 | 对照五个订阅 ID/topic/权限；公共 API 是 context.onEvent，不是 context.subscribe。 |
| parent.document 可读 / Tauri globals 可见 | 记录边界风险并停止隔离验收，检查宿主 sandbox/全局隔离，不能因 mock 测试通过而忽略。 |
| 资源加载失败 | 查路径、入口、扩展名、CSP、symlink/traversal 拒绝；不能加 allow-same-origin 或禁用安全策略来修复。 |

## 验证

以下是可重跑检查与待授权清单，**不代表已经全部实测通过**。真实执行结果记录于交付报告。

非桌面检查：

```sh
node --check src-tauri/crates/se-plugin-example/ui/app.js
npx vitest run src/renderer/features/plugins/web/shim.test.ts src/renderer/features/plugins/web/api.test.ts
```

此外用 DOM/模拟 shim 检查 startup summary、increment/reset 命令、raw onMessage、theme Promise、五个 context ID/resync、配置、notification DTO、CAS 缺失 revision/保存/冲突草稿保留、direct fetch URL/CORS/credentials、断连布局和 pagehide cleanup。模拟无法替代 native 存储、服务清理或真实桌面 IPC。

另行授权后的桌面验收：

- [ ] 三个 native 入口打开 activity-studio，旧 Counter 正常；指标与图表对应权威快照。
- [ ] increment/reset/summary 和通知正常；service-ping 不阻塞 UI，后台结果推送。
- [ ] terminal 创建/关闭/切换、project 切换、agent 状态事件与 resync 正常。
- [ ] 笔记跨关闭重开持久；两个 view CAS 冲突保留草稿；保存中编辑不被清空。
- [ ] 实时明暗主题、窄宽布局、键盘与屏幕阅读器状态可用。
- [ ] 真实 iframe 中 native ping 与 direct GET 分开验证，记录服务 PID/端口/实际响应头。
- [ ] 在 WKWebView/WebView2 记录 window.origin、parent.document 拒绝、Tauri globals 缺失及平台/宿主版本，不混淆 URL origin。
- [ ] 导航/重载、禁用、升级、卸载导致旧 session 拒绝；native 正常；禁用/卸载后服务退出，旧端口拒绝连接。
- [ ] 对应平台复测 CSP、loopback 与 sandbox；未测试平台明确标为未验证。
