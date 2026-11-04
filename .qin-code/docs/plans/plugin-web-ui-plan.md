---
schema: qin-doc/1
kind: plan
id: plugin-web-ui
title: 插件自带网页界面：编译后 dist + 原生后端，沙箱 iframe 与桥接
date: 2026-11-04
status: accepted
source: user:2026-11-04T09:32 "不做ts/js插件...只能安装编译后的 dist...但是界面可以随意设置"; agent://webui-planner
lang: zh-CN
scope: project
---

## 目标

插件后端仍是 Plugin Core 中的原生 C-ABI 库，不运行 Node/JS 后端。插件界面可以是任意 HTML/CSS/JS，以编译后的静态 dist 随插件安装。声明式 UiTree 视图与直接安装单个原生库的方式保持兼容。完整设计见 `agent://webui-planner`。

1. **安装包 `.seplugin`**：ZIP，内含 `manifest.json`、`package.json`（文件清单与 SHA-256）、`native/<target-triple>/<lib>`、`ui/`。流式解包时校验大小、条目数与压缩方式，拒绝路径穿越、符号链接、大小写冲突路径；包内 manifest 必须与原生库 ABI 导出的 manifest 完全一致。先完整发布到 `<profile>/plugins/packages/<id>/<version>/<archiveSha256>/`，再写注册表。SHA-256 只保证完整性，不代表发布者身份。
2. **manifest**：视图新增 `kind: "web"`、`entry`（`ui/` 下的相对 `.html`）与 `allow` 白名单（commands、view.dispatch、config.read/write、context.read/events、storage.read/write、feedback）。宿主侧能力仍需声明对应的 `se.host.*` 服务；新增保留服务 `se.host.ui@1`，供原生后端向页面推送数据。
3. **加载**：专用 `se-plugin` 协议，URL 带会话与代次，资产由 Plugin Core 读取并校验哈希，`no-store`。**CSP 宽松**（用户决定 2026-11-04 09:52）：允许内联脚本/样式与 eval，`connect-src`/`img-src`/`font-src`/`media-src`/`frame-src` 允许任意 `http(s):`、`ws(s):`、`data:`、`blob:`，包括 `localhost`；不再要求在 manifest 声明网络访问。跨源请求是否成功仍取决于目标服务的 CORS（页面是不透明源，`Origin: null`）。
4. **隔离**：`<iframe sandbox="allow-scripts">`，永不加 `allow-same-origin`，因此页面无法使用 Tauri IPC、读写主界面 DOM 或访问其他插件。文档被二次加载（导航）时撤销会话，并显示错误页。
5. **桥接**：宿主提供版本化的 `se-plugin-ui.js`，页面经 postMessage（nonce 握手、来源校验、限速、限大小、请求超时）调用 `se.invoke`、`se.view.dispatch`、`se.config`、`se.context`、`se.storage`、`se.feedback`。所有调用以插件身份记账，并在禁用、升级或断开时按代次失效。原生后端通过 `se.host.ui.postMessage` 推送数据。
6. **主题**：以数据形式下发宿主设计令牌与明暗模式，主题切换不会重置页面状态。页面外由宿主保留标题栏、信任标识和重新加载按钮。
7. **Core 能力协商**：新增可选能力 `plugins.web-ui.v1`。旧版 Core 仅对网页视图返回 `PLUGIN_NOT_SUPPORTED`。
8. **后台唤醒**（用户决定一并实现）：新增保留服务 `se.host.wake@1`。原生插件的任意线程可在回调之外调用 `request`（非阻塞入队、合并、限速），宿主随后经 `on_service_call` 投递 `se.host.wake.wake`，插件在这次回调内刷新视图、发通知、写存储、向网页推送。禁用、升级或隔离后失效。
9. **受管后台进程**（用户决定一并实现，不限制后端调用）：manifest `contributes.processes`，每项包括 `id`、`command`（包内相对路径，或绝对路径 / PATH 中的命令）、`args`、`env`、`cwd`、`autostart`、`restart: never|on-failure|always`。Plugin Core 在插件激活时按进程组启动，崩溃按策略退避重启；禁用、卸载、升级或 Core 退出时整组结束。stdout/stderr 进入有界日志环形缓冲。新增保留服务 `se.host.process@1`（status/start/stop/restart/logs），管理页显示状态与日志。插件内部可以访问任何本地或远程服务，主程序不做限制。

## 非目标

- 不提供 Node/JS 后端、开发服务器例外，也不支持远程 URL 视图。
- 不做独立原生 webview 窗口。
- 不声称沙箱能约束原生代码：原生插件仍是完全受信任代码。
- 不限制插件原生代码与受管后台进程的网络、文件、进程访问：这属于插件内部行为。
- iframe 沙箱只阻止页面使用主程序的 Tauri IPC 和主界面 DOM，不作为网络边界。

## 任务

依赖顺序：W1 → (W2 ∥ W4) → W3 → W5。W1 同时冻结后台唤醒与受管进程的契约；W2 在网页包之后实现两项后台能力，W4 在管理页展示进程。每个文件只有一个负责包，同一时间只运行一个 cargo 进程。

| 包 | 内容 | 主要文件 |
|---|---|---|
| W1 契约 | manifest 字段、package/bridge DTO、schema、共享 fixture、`plugins.web-ui.v1`、依赖（zip、base64、mime_guess） | `se-plugin-protocol/**`、`src/shared/types/plugin*.ts`、`contracts.json`、Cargo 清单 |
| W2 宿主 | 归档暂存与原子发布、资产校验、web 会话生命周期、`se.host.ui`、打包 API `write_package` | `se-plugin-host/src/**` |
| W3 传输 | 5 个 RPC、Tauri 命令、`se-plugin` 协议与 CSP、隔离探针 `--plugin-ui-probe`（仅调试构建） | `src-tauri/src/core/plugin.rs`、`plugin_commands.rs`、`lib.rs`、新 `plugin_ui_protocol.rs` |
| W4 渲染 | `PluginWebView`、桥接、主题、shim、加载/错误/重载状态，安装选择器支持 `.seplugin` | `features/plugins/web/**`、`PluginViewPanel/Page`、管理页 |
| W5 示例与证明 | Activity Studio 示例界面（指标卡、SVG 图表、时间线、笔记），打包 CLI，e2e 测试，原生 webview 隔离探针，作者 runbook | `se-plugin-example/**`、`plugin_web_ui_e2e.rs`、`scripts/test-plugin-web-ui.sh` |

## 验收

- W1：契约测试通过，旧 fixture 依旧有效，新 fixture 在 Rust 与 TS 中分类结果一致。
- W2：`archive_rejects_escape_and_alias_paths`、`archive_hash_failure_never_loads_native`、`archive_manifest_must_match_abi`、`archive_publication_is_atomic`、`legacy_registry_and_library_still_load`、`web_session_revokes_with_generation` 等测试通过。
- W3：旧会话无法在重连后复活；GET/HEAD 正常、非法路径失败、已禁用插件返回 404。
- W4：侧栏与面板显示插件自己设计的全尺寸界面；伪造来源或 nonce 的消息不会触发任何调用；浏览器端无副作用。
- W5：原生探针报告 `child_native_side_effects:0`、`parent_dom_unchanged:true`、`old_session_rejected:true`，并在隔离实例中通过 cua-driver 实测。
- 全部 cargo 命令带 `--target-dir /Users/qs/project/me/termul/src-tauri/target`。
