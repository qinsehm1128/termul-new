---
schema: qin-doc/1
kind: plan
id: mcp-source-isolation
title: Se内置MCP与外部聚合器双入口实施
date: 2026-11-07
status: draft
source: Confirmed user boundary plus read-only routing/entry exploration and split-implementation-plan
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"33"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"34"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"35"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"36"}],"goal_refs":[]}
---

## 目标

实现用户确认的两个独立MCP连接：`se-mcp`仅Se内置session-memory/workspace/SSH，`se-mcp-aggregator`仅外部上游。列表、调用、resources/prompts、控制查询、动态重载与ACP注入均隔离。source与grouped/entry/direct呈现方式正交。保留记忆数据、SSH授权/黑名单和se-memory退役。

实现选择：采用两个不可变来源McpCore及独立service/settings/端口/token，仍共用同一个se-mcp binary；不把“两个工具”误实现为同一个混合集合的显示开关。详细只读规划证据见agent://split-implementation-plan。

## 非目标

不新增binary或业务工具，不重写SSH/记忆业务、不改变PTY生命周期，不泛化改造所有外部命名/OAuth，不新增Web凭据导出，不修改真实用户配置、不启动真实上游或SSH，不自行发布或安装。不复制两份上游控制文档，继续使用现有mcp-servers.json及revision规则。

## 任务

### 冻结契约

- Rust Source统一定义在se-mcp-bridge/settings.rs并从crate root导出：Internal/Aggregator，serde小写，默认Internal，提供as_str/parse/client_key/default_file_name。
- settings schema2含必填source。旧v1缺字段只解释为internal；v2缺source、未知版本/来源或期望来源不符拒绝。迁移保留旧internal token/port，aggregator另生成，不在bridge只读过程中写盘。
- internal文件mcp-gateway.json（debug .dev.json），aggregator文件mcp-gateway-aggregator.json（debug .dev.json）；端口base分别3290/3390，继续加debug+2*canary，避免渠道冲突。宿主descriptor仅internal。
- CLI `se-mcp --source internal|aggregator --mode grouped|entry|direct [--config PATH]`。旧无source收窄到internal。两个客户端key固定为se-mcp与se-mcp-aggregator，分别sync/unsync且保留另一条及第三方配置。
- 每个纯来源端口保留现有mode路径及/mcp/direct别名，不保留混合端口。identity路由`GET /mcp-core/identity`无需凭据，返回protocol=se-mcp-gateway、protocolVersion=1、source。
- bridge、ACP及生命周期检查先验证来源identity。bridge每次发送bearer/MCP前检查，缺marker/错source不得回退到旧混合服务。HTTP clients禁用proxy/redirect。--url只允许选中settings的literal-loopback HTTP同origin及选中mode路径，无userinfo/query/fragment。
- 请求/响应使用X-Se-Mcp-Source与X-Se-Mcp-Protocol-Version。服务拒绝出现但不匹配的marker；正常直接HTTP客户端仍可用本来源bearer连接。
- ServiceStatus强制source/protocol/protocolVersion；ServiceView含source，Connection含source/httpReady。stdio可离线导出，HTTP片段仅验证通过后启用。生命周期legacy替换仅允许已认证且严格识别的internal旧daemon，不能业务转发或误停未知监听者。
- `McpCore::new_internal(config,permission,builtins)`与`new_aggregator(config,permission)`，无Mixed。旧new/Default解释为aggregator，new_with_builtins解释为internal。变更入口及dispatch都有来源防线，internal禁止连上游，aggregator禁止注册builtin。外部上游可同名workspace，来源不能靠字符串黑名单判定。
- 每个source独立reload/snapshot fence。internal仅投影builtIns，不解析上游secret；aggregator仅投影upstreams及描述/rejected状态。语法损坏保持各自last-good。资源/prompt无法从internal调用上游。
- 未被生产使用但可启动第三listener的env supervisor child退役：无--config的--mcp-core固定提示并退出2，不启动GUI/listener。
- McpServices持有两个Arc<McpService>，提供get(source)、reload_all、set_port(source,port)。单源端口冲突/变更失败不得停止另一源，失败恢复原来源设置。
- 现有10项Tauri命令添加可选source，缺省internal，非法值MCP_SOURCE_INVALID：runtime status、start/restart/set_port、connection、clients detect/sync/unsync、servers/server_tools。全局put config和remote sync双reload，OAuth及上游描述显式aggregator。
- ACP按source保存文件/endpoint，生成两个router名mcp-router/mcp-router-aggregator。new/replacement/load/resume均验证身份，文件无效不得回退缓存端点，一个来源失败不隐藏另一个，不能持同步锁跨await。
- standalone采用双embedded service和source-aware watcher，与Web写入同一配置。旧TERMUL_MCP_CORE_*归internal，新增TERMUL_MCP_AGGREGATOR_*；旧CORE_ENABLED=false关闭整体。双bind失败清理本次已启动服务，token/非零端口不得相同。shared-live复用桌面，不再启动额外listener。
- UI两套来源状态/端口/mode/client列表与两张导出卡。新mcp/gateway-modes保存两份mode，旧gateway-mode只迁移到internal。异步按来源和请求序号隔离；描述页固定aggregator。Web/phone本机stdio导出继续desktop-only、零invoke。stdio输出不包含token，日志只含source/operation/稳定错误码。

### 文件所有权与依赖

1. Wire owner先实现bridge crate `src/{settings,bridge,lib}.rs`，发布Source/Identity API并跑crate tests。
2. Domain owner：`mcp_core/{domain,http,gateway_tools,snapshot,process,mod}.rs`及`tests/{mcp_core_domain,mcp_core_http,mcp_core_memory_merge,mcp_core_process}.rs`。按冻结Source API编码，wire完成后编译。
3. Service owner（wire完成后继续）：`mcp_core/{service,clients}.rs`、`acp/{manager,mcp_router}.rs`、`tests/mcp_gateway_service.rs`。实现双service、client锁和ACP双router，不碰主会话集成文件。
4. UI owner：新shared/types/mcp-gateway.types.ts，renderer gateway api/store、GatewayPanel/GatewayConfigSection/DescriptionsPanel/ControlPanel、对应tests与en/zh-CN mcp.json。
5. 主会话：`src-tauri/src/{lib,commands,main,server_main}.rs`、`core/{acp,app_host}.rs`、新tests/mcp_source_isolation.rs、renderer parity-checklist及plugin catalog。验证remote/host、Web routes、两个App根适用接线，不新增Web token导出。

交叉编译在各lane合并后统一判定，不以临时Mixed stub压过编译错误。直接teammate分工，不使用work或额外token配额。

## 验收

- `cargo test --manifest-path src-tauri/Cargo.toml -p se-mcp-bridge`
- `cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --features mcp-test-fixture --lib mcp_core::`
- `cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --features mcp-test-fixture --test mcp_core_domain --test mcp_core_http --test mcp_core_memory_merge --test mcp_core_process --test mcp_gateway_service --test mcp_source_isolation`
- `cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib acp::mcp_router::`及`--lib acp::manager::tests::`
- `cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --features standalone-server --bin se-server mcp_source_`，必须实际执行新增拓扑/重载/失败回收测试，不能以0测试通过代替。
- `cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib remote::host::`
- `cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --test mcp_host_web --test mcp_host_preview_web --test mcp_capabilities_acceptance --test workspace_mcp --test se_memory_removal`
- 指定gateway api/store、两配置/连接panel、描述页、ControlPanel及parity Vitest；完整typecheck、Tauri/Web双构建。
- desktop lib/se-manager/se-mcp与standalone no-default-features构建检查；scoped rustfmt/Biome、catalog unclassified:0及git diff --check。
- 覆盖source×mode与direct别名、entry四个方法、跨source调用计数零、交叉token拒绝、resources/prompts、动态重载、同名外部provider、双客户端并发同步、旧daemon fail-closed及隐私sentinel。所有测试仅使用临时目录、loopback fixture和假凭据，不访问真实上游或SSH。
