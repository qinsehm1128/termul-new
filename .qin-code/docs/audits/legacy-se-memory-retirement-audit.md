---
schema: qin-doc/1
kind: audit
id: legacy-se-memory-retirement
title: 独立se-memory退役验收
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: User retirement request, disjoint implementation, live configuration/process verification and independent analysis
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"27"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"28"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"29"}],"goal_refs":[]}
---

## 结论

按用户最新要求退役独立 `se-memory`，替代上一轮保留兼容卡的决定。源码已移除独立stdio实现、两项invocation命令与core IPC分发、前端导出卡/helper/API成员和旧文案。统一 `se-mcp` 的session-memory、workspace、SSH及已有记忆索引保留。

本机3条旧启动配置已经备份并定向移除，旧memory-only进程清零。原共享网关PID75355与桥接PID52727未重启。源码尚未打包发布或安装。

## 发现

- `memory_index/stdio_mcp.rs`并非统一网关共享实现，可整文件删除。保留memory_index service/store/scope/paths及mcp_core BuiltInRegistry的记忆provider和ACP内部记忆工具。
- 两个binary main第一步拒绝退役参数`--memory-mcp-server`，输出固定迁移提示并退出2，防止旧配置意外打开GUI或启动服务器。仅保留这段拒绝逻辑，不保留旧服务实现。
- 移除两项Tauri invocation命令、两个core IPC方法以及renderer/shared对应成员。MCP页仅保留完整se-mcp导出。Web/desktop六项记忆API及其参数/返回契约继续测试。
- 完整memory_index测试发现既有fixture问题：原祖先项目恰好包含私有state目录，触发正确的STATE_ROOT_INSIDE_PROJECT保护。仅将该测试的project嵌套一层以测试合法祖先的namespace隔离，并新增包含私有state的祖先必须拒绝测试。生产隐私防护未改变。

## 证据

### 代码与回归

- 主会话Rust验收任务`348089yptd`退出0：memory_index 189 passed、0 failed、1既有ignored；core::acp 11 passed；mcp_core_memory_merge 7 passed；se_memory_removal 5 passed。desktop se-manager与headless standalone se-server编译检查通过。
- 主会话前端验收任务`010344b742`退出0：6文件196测试、完整typecheck、Tauri/Web双前端构建通过。覆盖统一导出、旧入口缺失、双传输记忆API、parity及既有连接页。
- catalog checker：327 registered commands，unclassified:0，较原329删除两项旧导出能力。git diff --check通过。
- 独立`legacy-removal-boundary`静态分析无阻塞问题，确认入口先拒绝、暴露面配套删除、统一能力/数据路径保留及fixture只改测试。分析者未独立运行测试。
- 生产源码残留检查：renderer旧名仅出现在负向测试，Rust仅在两个main的固定拒绝分支保留旧flag与迁移提示。无旧stdio实现、invocation命令或UI卡片引用。

### 本机配置与进程

清理前确认的条目：
- `~/.se-manager/mcp-servers.json` 两个旧stdio上游，ID `8c4d7f6c-aa0a-4359-9b5d-4354cd9d0337`、`d184a0f0-d526-45ba-b7a7-c9d5ceaf7a13`。
- `~/.qin-code/mcp.json` 的 `servers.se-memory`。

私有备份：`~/.se-manager/backups/legacy-memory-retirement-20261107-183740/`，目录0700、文件0600。通过已确认ID+binary+旧flag筛选，保留完整原配置副本，原始字节变更检测后原子替换并递增Se registry revision。未通过管理API写入，未修改网关token/端口配置。清理后与备份精确比较，仅两条上游、一条客户端配置与revision变化，其他字段包括builtIns/routing/环境值不变。

现有统一连接实际来自`~/.claude.json`的se-mcp条目，逐字节保持不变。只读检查的Claude、Codex、Cursor、OpenCode配置没有旧flag条目。

网关热加载后两个旧子进程自行退出。qin-code disconnect旧se-memory后仍残留PID52889，复核binary、旧flag和父PID后仅对该进程发SIGTERM，并监听到退出。18:39:35 +08:00复查：0个旧memory-only、1个gateway、1个bridge，原网关/桥接PID未变。没有killall，也未停止PTY或其他Se组件。

统一入口再次仅执行initialize/tools/list：聚合工具由284降至274（删除两份各5工具的旧上游），22个Se内置工具全部仍在，包含6项记忆/边界能力、3项workspace和13项SSH。

## 残留风险

- 新binary的退役参数行为由严格入口AST/source回归和desktop/standalone编译验证，未执行新binary运行烟测。当前安装0.15.34的文件未替换，源码级入口移除需后续新构建；实际旧配置和进程已清理。
- 未删除或重建任何用户记忆数据，未执行真实SSH工具。统一查询行为由隔离fixture集成测试验证，live实例只列工具、不读取用户会话内容。
- 配置操作使用变更检测、revision递增及原子替换，不宣称是与所有外部写入者互斥的事务。事后完整配置比较确认此次没有其他字段变化。
- 保留旧审计作为历史记录，不将其“保留兼容入口”结论视为当前设计。用户.gitignore和scripts/release/__pycache__未提交或清理。
