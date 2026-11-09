---
schema: qin-doc/1
kind: audit
id: se-mcp-workspace-ssh-verification
title: Se MCP 扩展集成验收记录
date: 2026-11-07
status: accepted
source: Goal mechanical verify 2026-11-07 13:26+08:00；本会话测试任务5162622zf9/643352bemo/6433990anz/8481941rm9；agent://mcp-final-boundary-review；agent://ssh-runtime-impl；req-a1fec49128c8a770f1a9b055e56197e7
updated: 2026-11-09
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"6"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"7"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"8"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"9"}],"goal_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"goal-0d7e8bfa-8555-4bc5-8aa1-d77f2afd84fe"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"goal-bea3315f-0654-4818-9cd1-a87976268802"}]}
---

## 结论

本轮实现与机械验收完成。2026-11-07 13:26+08:00，Goal verify 使用未削弱的原完整命令返回 passed。交付为 session-memory 六工具、workspace 三工具、ssh 十三工具，以及宿主授权、私有桥、桌面/shared-live/standalone/Web 共用接线。提交只包含本任务文件。

## 发现

### 已实现与收敛

- project-scope 两工具并入 session-memory，隐藏旧别名仍可调用；迁移工具继续受旧/新 policy 与 ToolPermission 交集约束，不扩权。前端保留 built-in 与三种 upstream transport 的 policy，非法 policy fail closed。
- FileProjectRegistry v4 承载分组，workspace 从现有镜像查询无记忆项目。ready 与空集合区分，按授权过滤后再做分组/计数/分页；路径另行授权，缺省项目范围为空。
- SSH 13 操作使用既有 profile source 和宿主 credential resolver。strict known-host key 拒绝未知/变化/不可读记录，不自动 enrollment；专用连接不借用户 PTY。执行双流共享预算，文件只支持受限单文件与 exclusive 新建，不承诺覆盖、cwd、expectedHash 或 jump host。
- global HostAccessPolicy 默认关闭，CAS 持久化并绑定 profile 身份指纹。所有 MCP 客户端共享 global 主体，不宣称多租户隔离。
- 私有 bridge 只绑定 loopback，独立 token 与用户私有 descriptor，拒绝 proxy/redirect，限制请求/响应。认证的 UUID 取消通道有容量上限与提前取消处理；显式 cancellation 和 dropped future 均有回归证据。
- 后台任务有运行中有界输出、稳定 offset、owner/profile/pin 检查、撤权取消监视、私有 atomic store 与跨进程 writer lease。重启将旧活动任务恢复为 interrupted，并保留远端终止未知。只恢复观察，不重放命令、重连接管或盲杀 PID。
- desktop 与 standalone 经 production_host_ssh 接入专用 0700 mcp-ssh-jobs/jobs.json。Unix 强制使用持久化，初始化失败不暗中降级；非 Unix 保留基础目录/SSH 服务，job_* 显式返回 UNSUPPORTED_PLATFORM，不阻断整个宿主启动。
- 三个 Tauri command 与三个 HTTP twin 已接线。HTTP 强 LocalOperator provenance，PublicTunnel 连授权候选查询也拒绝。shared-live 复用 desktop service Arc，standalone profile 与 web 写入复用同一 WebStore Arc。
- UI 不再在保存其他设置时刷新已授权 profile 的指纹。新选中或专门的重授权按钮才更新 pin；删除 profile 的孤立 grant 在保存时清理，避免无法重新启用。保存提示与行为已一致。

### 审查与失败修复

独立 analyst 两轮发现并已修复：job profileId、SSH offset 分页、schema 的不支持参数/范围、stale-profile 全局关闭、UI 隐式重授权、孤立 grant 与旧文案。reviewer 模型 route 曾因认证 503 未执行，未当作审查通过。

最初 bridge 取消测试等待已丢弃的 handler future 继续发 notify，不能观察实际取消；改为在独立 watcher 中观察传给后台执行器的 token 后，两条 token/drop 测试通过。最终 Goal 首次执行因 macOS TMPDIR 的 /var symlink 被严格 no-follow store 拒绝而失败；fixture 改用自己的 canonical 实际目录，保留生产 no-follow 安全检查。恢复验收轮次后原完整命令通过，未删除或削弱测试。

## 证据

### 最终机械验收：通过

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --test mcp_core_memory_merge --test workspace_registry_groups --test mcp_capabilities_acceptance
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib mcp_core::
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib ssh::
cargo check --manifest-path src-tauri/Cargo.toml -p se-manager --features standalone-server --bin se-server
bun run typecheck
bunx vitest run src/renderer/lib/mcp-api.policy.test.ts src/renderer/lib/__tests__/parity-checklist.test.ts
python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py
```

### 补充验收：通过

- 5162622zf9：MCP 库 89 tests，包括私有桥认证、重启 discovery、提前取消、token 与 future-drop；acceptance/memory/groups/workspace/web 集成与修正后的 gateway service 五项通过。
- 643352bemo：SSH 库 64、project_registry、SSH integration 15，以及 `--no-default-features --features standalone-server --bin se-server` check 通过。
- 315578jybu：既有 MCP domain 7、HTTP 4、process 2 通过。最初 gateway 列表断言因新增默认 provider 失败，随后完整列表及 HOST_UNAVAILABLE 回归在 5162622zf9 通过。
- 6433990anz：六个前端测试文件 239 tests、完整 TypeScript、Tauri renderer 与 Web 两种生产构建通过。
- 8481941rm9：最终 UI panel 13 tests 和七项宿主 integration 通过。后续仅修正 fixture canonical 路径，已由完整 Goal 再验证。
- mcp_host_web 五项包括真实 serve_router 同实例回归通过。SSH 私有文件、writer lease、重启恢复和 live progress 用临时 fixture 验证。
- catalog 最终 `registered commands: 328; unclassified: 0`。原目录保全检查通过。新增命令、HTTP twin、内部 call/cancel、renderer state 和 credential 边界同步在同一交付。

此前小步提交 61a28682（policy 往返修复）已有专属测试与独立 review。最终扩展提交信息见仓库提交记录。

## 残留风险

- 没有连接真实 SSH/SFTP 主机，没有真实远端 process-group 取消实验，没有手动桌面/手机 UI 操作验收。Windows/non-Unix 分支未在对应平台执行，虽有显式 unsupported 测试分支，不能声称已跨 OS 实测。
- 持久任务要求 Unix Se 宿主。恢复为 interrupted 只保留观察数据，远端进程可能仍运行。进程组取消只保证已确认的专用组，不能保证主动逃离进程组的后代；普通 execute 超时/断通道也不证明远端终止。
- SFTP v3 与路径 API 对恶意并发 ancestor rename 存在 TOCTOU 残余。应使用低权限账户与可信根目录。失败的新文件写入可能留部分文件。DNS/fsync 等 OS 调用不能硬中断，开始的文件操作不能回滚。
- 输出快照有 250ms 节流，崩溃前极短一段日志可能未落盘；异步持久化错误以 storeError 显示。
- `.gitignore` 与 `scripts/release/__pycache__/` 为用户原有变更，保持未提交，不属于此交付。
