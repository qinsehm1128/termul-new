---
schema: qin-doc/1
kind: audit
id: mcp-entry-display-verification
title: MCP完整入口展示与复制验收
date: 2026-11-07
status: superseded
updated: 2026-11-09
source: Live process snapshot, installed protocol probes, UI implementation, tests and independent boundary analysis
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"24"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"25"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"26"}],"goal_refs":[]}
---

## 结论

MCP服务器页已优先展示完整 `se-mcp` 直连配置卡，支持MCP JSON与Codex TOML预览/复制；原 `se-memory` 保留为明确标注的仅记忆兼容入口。复用既有connection API与配置构造器，不改变连接Agent页模式、授权或服务生命周期。此次仅提交源码，不创建新版本或安装。

## 发现

- 原截图是上游服务器 `se-memory` 的工具列表。安装版0.15.34的 `--memory-mcp-server` 在隔离HOME/state-root下实测仅5个记忆工具。安装包内 `se-mcp --mode direct --config ...` 实测返回284个聚合工具，其中22个Se内置工具（memory6、workspace3、SSH13）。仅执行initialize/tools/list，无工具调用。
- 新主卡在内置开关、授权卡、上游列表和旧记忆卡之前。固定明确标注直连全部内置及已启用上游工具，提示可见不等于授权，并禁止将网关导回自身上游列表。
- 完整导出只提取并复制bridge command/args，不输出HTTP token或原始错误。旧记忆入口不自动扩权。生成失败不再回退到旧预览，剪贴板失败不会报成功，格式切换与卸载使旧UI请求失效。
- 浏览器、shared-live和standalone Web明确显示本机stdio导出仅桌面可用，禁用预览/复制，无Tauri调用。既有连接Agent页HTTP功能不变。
- 2026-11-07 18:11:02 +08:00实际进程快照：1个共享gateway（75355）、3个memory-only（52889、76586、77869）、1个stdio bridge（52727）。bridge和一个memory由qin-code启动，另外两个memory由gateway作为上游启动。工作区及SSH为网关内置模块，不是单独进程。此计数不包含第三方上游服务，且随客户端连接变化。

## 证据

- 主会话最终回归任务 `096164nplp` 退出0：7个Vitest文件203测试通过，完整typecheck、Tauri/Web双前端构建、8文件Biome及git diff --check通过。
- 覆盖文件：McpControlPanel.test.tsx、McpGatewayConfigSection.test.tsx、MemoryMcpConfigSection.test.tsx、McpGatewayPanel.test.tsx、mcp-client-config.test.ts、mcp-gateway-api.test.ts、parity-checklist.test.ts。
- 插件目录检查329 registered commands、unclassified:0。本次不新增command/route/store，更新既有getConnectionInfo条目的导出边界说明。
- 原reviewer因provider认证不可用未执行。替代独立analyst `mcp-entry-boundary-analysis` 完成只读代码检查，无阻塞结论，未自行运行测试。
- 隔离Vite fixture使用固定bridge、虚构token及内存剪贴板，不加载用户配置。Ego浏览器验证真实中文卡片、JSON及TOML复制、格式切换清除旧预览、token sentinel未显示、browser禁用按钮。
- 390px grid fixture发现代码预览固有宽度撑开卡片，已给两张卡加min-w-0。复测main client/scroll width均390，两张卡均356/356；长代码仅在预览框内滚动（313/530）。修复后重新运行完整验收链通过。预览服务和3个自建页面均已关闭。

## 残留风险

- 未测试真实桌面系统剪贴板，浏览器交互使用内存mock。请求失效保护UI和通知，但无法撤销已经提交给系统的剪贴板写入。跨请求原生写入顺序存在未实测的低风险限制，不声称严格latest-wins。
- 既有connection IPC仍含token并进入renderer调用栈，新卡只提取bridge保证输出边界，不声称token完全不跨IPC。既有prepare_settings首次可能准备settings/token，不声称导出绝无写盘。
- 未进行真实SSH或用户PTY操作，没有自动改变授权、客户端配置、服务数量或运行状态。用户.gitignore和scripts/release/__pycache__保留。
- 新界面尚未打包发布或安装，已安装0.15.34不会因源码提交而自动变更。
