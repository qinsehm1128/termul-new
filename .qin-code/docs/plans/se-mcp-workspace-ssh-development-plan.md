---
schema: qin-doc/1
kind: plan
id: se-mcp-workspace-ssh-development
title: Se MCP 完整扩展实施计划
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: 用户批准req-a1fec49128c8a770f1a9b055e56197e7；agent://mcp-implementation-plan；当前实施接口
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"5"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"6"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"7"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"8"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"9"}],"goal_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"goal-bea3315f-0654-4818-9cd1-a87976268802"}]}
---

## 目标

实现req-a1fec49128c8a770f1a9b055e56197e7批准的完整MCP扩展：将project-scope的两工具合并进session-memory，保留旧调用和旧策略不扩权；增加workspace三工具与ssh十三工具，包括后台任务。单se-mcp入口，规范业务工具合计22个。

以实际实现中的接口为准，不强制照搬规划agent的尚未落地DTO。原分析：qin-doc:se-mcp-workspace-ssh-expansion。用户在2026-11-07 11:20+08:00批准完整实施并提出provider合并。

## 非目标

不连接真实SSH主机、不读取用户秘密、不接管用户PTY。不新增SSH配置CRUD、任意凭据读取、jump host、端口转发、递归传输、项目到SSH启发式关联。不开放公网网关，不宣称多租户隔离。

## 任务

### 已冻结实施契约

- session-memory仅一个公开provider，六个记忆/范围工具；legacy project-scope只做隐藏兼容，两个迁移工具同时受新旧policy与ToolPermission约束。
- workspace三工具：group_list/project_list/project_get。实际ProjectRegistry为权威镜像，FileProjectRegistry升级v4承载groups。查询默认拒绝，按授权先过滤再计算数量、分页与组成员，路径单独授权。ready与真正空集合区分。无记忆项目也可见。
- ssh十三工具：profile_list/profile_get/test_connection/execute/list_files/read_file/write_file/upload/download/job_start/job_status/job_list/job_cancel。使用profileId，job操作按jobId。真实libssh2 backend与fake测试后端分离，不借用UI连接。
- 共享接口：mcp_core::host_capabilities::HostCapabilityBackend::call(provider,tool,Map,CancellationToken)->Result<Value,HostCapabilityError>。desktop经私有loopback bridge，standalone本进程调用HostCapabilityService。
- 宿主授权HostAccessPolicy为schemaVersion/revision/enabled/workspace/ssh/profileFingerprints。默认enabled=false，workspace grant默认空，按profile逐operation授权，fingerprint绑定连接身份。CAS写入私有policy文件。MCP所有客户端共享本宿主global主体，不声称caller隔离。
- SSH ProfileSource与CredentialResolver注入既有数据源，不复制配置数据库。实际credentials.rs后端是用户私有文件，旧OS keychain注释已过时。桌面捕获已有backend，standalone必须显式配置凭据文件，否则只能使用该宿主agent或不需口令的key。
- 独立bridge仅127.0.0.1、与公共MCP token不同的随机token，私有descriptor随重启刷新；拒绝redirect与环境proxy，限制请求/响应大小；host离线返回HOST_UNAVAILABLE，不返回假空清单。
- Tauri/web对偶：mcp_host_access_get / GET /mcp/host/access；mcp_host_access_put(expectedRevision,policy) / PUT同路由；mcp_host_call(provider,tool,arguments) / POST /mcp/host/call。Web必须经实际provenance和鉴权，PublicTunnel禁管理/主动执行。
- UI在共用MCP设置页面，显式选项目/SSH profile、操作和路径范围，明示shell是远端账号权限不是路径沙箱。本地传输路径是Se宿主路径。
- 桌面项目镜像同步不能依赖shared-live已开启。standalone WebStore需同一Arc实例供web写入与MCP读取，避免独立缓存陈旧。
- timeout/drop SSH channel不等于远端进程终止，输出terminationConfirmed/unknown。后台任务完整生命周期与恢复能力须以实现/测试证据判定，不以工具schema作为完成。

### 文件所有权

| owner | scope | 状态 |
|---|---|---|
| memory-merge-impl | mcp_core builtins/config/domain/gateway_tools/snapshot及memory合并测试 | 已实现合并，追加workspace/ssh默认与保留ID |
| workspace-registry-impl | acp/project_registry、web/project_registry、mcp_core/workspace及对应测试 | groups底座完成，provider/ready在实现 |
| ssh-runtime-impl | src-tauri/src/ssh/**和ssh_automation测试 | 13操作真实runtime、jobs、限额/授权 |
| mcp-access-ui | renderer/shared TS、MCP设置、locales、项目同步及回归 | UI/facade/权限normalize |
| mcp-web-wiring | server_main、web/mod/router/mcp_host_api、remote/host与web测试 | standalone/shared-live装配 |
| mcp-catalog-sync | 仅plugin-capability-catalog-spec.md | 同步新命令/路由/状态分类 |
| 主会话 | mcp_core host_*、mod/service/process，lib/commands，mcp_capabilities_acceptance测试 | 桥/授权/宿主组合/桌面接线/验收 |

所有执行器不提交，主会话按scope验证后提交。禁止全仓cargo fmt覆盖并行修改。

### 执行器回退记录

尝试managed work mcp-memory-merge，但launch一直未生成immutable_base/controller/executor，公开delegate/reconcile schema无法驱动，CLI无work子命令。已通过work pause成功冻结revision2，防止恢复后双写。改用普通受scope约束的teammate在main共享工作区实现。预建refs/heads/feat/mcp-workspace-ssh-20261107仍在初始08ed71e1，不是已集成证据，不自行创建worktree。

### 已完成小步

- 08ed71e1：分析文档。
- 61a28682：前端MCP内置policy规范化不再丢allow/deny，invalid fail-closed。15个policy/OAuth测试、全量TS hook、独立review通过。
- 既有MCP基线79单测通过；记忆执行器随后报告84通过，但并行代码后变动需最终重验。
- 主会话2026-11-07 12:10+08:00 cargo check --lib通过（有既有deprecated警告）。这不是完整功能验收。

## 验收

Goal已配置机械验收：

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --test mcp_core_memory_merge --test workspace_registry_groups --test mcp_capabilities_acceptance
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib mcp_core::
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib ssh::
cargo check --manifest-path src-tauri/Cargo.toml -p se-manager --features standalone-server --bin se-server
bun run typecheck
bunx vitest run src/renderer/lib/mcp-api.policy.test.ts src/renderer/lib/__tests__/parity-checklist.test.ts
python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py
```

还需所有新增workspace_mcp/ssh_automation/mcp_host_web/前端panel/facade测试及适用build，独立安全与集成review。mcp_core_domain/http/process使用mcp-test-fixture特性验证，不能把缺fixture导致失败跳过。UI observable验证需如实报告环境限制。

验收覆盖：权限不扩张矩阵；旧别名与三模式一致；无记忆项目可见；未初始化/空/未分组/归档；目录数量无越权泄露；未授权零联网；凭据sentinel不泄漏；host-key未知/变化拒绝；双流/非零/超时/超量；路径穿越和symlink/覆盖/temp清理；job所有权与取消未知；重启不杀用户PTY；四端同服务同策略；目录unclassified:0。

隔离SSH动态fixture若不可用必须报告not_run，不得用真实用户profile代替。未通过完整验收前，Goal保持运行，不能将未完成的后台作业持久恢复或安全原子写入宣称已支持。
