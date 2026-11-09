---
schema: qin-doc/1
kind: audit
id: mcp-host-access-ux
title: MCP宿主授权面板体验优化验收
date: 2026-11-07
status: accepted
updated: 2026-11-09
source: 用户授权面板优化与默认全权限要求；req-1542e4731165f87cace3f1731e1a65d3；任务903806lh0a；host-ux-review与host-ux-runtime-debug结果
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"13"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"14"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"15"}],"goal_refs":[]}
---

## 结论

完成宿主授权面板的紧凑分区、搜索、批量选择和折叠优化，并落实用户追加要求：勾选「授权全部操作」时，默认选中该 SSH 配置的全部 13 项操作权限。已有授权不自动扩大。最终六个测试文件 214 项通过，TypeScript、桌面/Web 构建和能力目录检查通过。本轮不发布新 tag、不替换本机运行中的应用。

## 发现

- 工作区与 SSH 分区分别可收起，标题显示已选/已授权数量。SSH 卡片默认折叠，授权时自动展开，可展开全部/收起全部。
- 项目列表支持名称/ID 搜索、仅看已选、全选当前结果、清空，桌面双列、手机单列，列表限高约 208px。筛选全选保留筛选外已有选择，只生成当前 ID 集合；未来项目授权仍须单独勾选 null scope。
- SSH 搜索支持配置名、主机、用户和标签。卡片显示连接摘要及权限数量，读取/连接/高风险操作分组，安全说明可折叠。
- 初次授权按用户明确要求选择全部 13 操作。标签和说明明确包含高风险操作。全局开关、workspace 范围、根目录、POSIX 许可及最终保存仍独立，不自动调用远端。
- 「设为仅只读」替换为六个只读操作，能收窄已有全部权限。「清空权限」和逐项调整仍可用。已有配置上手动全选高风险操作须确认。
- 搜索/折叠/批量修改不会刷新身份指纹，不会删除筛选外 SSH 授权。仅新授权或明确身份重授权动作更新 pin。PublicTunnel 禁止修改及主动调用。
- 新增状态仅为组件内的筛选/展开/确认状态，非持久化新设置。宿主 schema、Tauri/web command、事件与插件操作不变，仍经既有 mcp_host_access_put 保存策略。

## 证据

- 最终命令任务 903806lh0a：panel、真实 i18next runtime、host facade、MCP policy、parity checklist、locale resources 六个文件共 214 tests passed。
- 同一任务完整 `bun run typecheck`、`bun run build:frontend:tauri`、`bun run build:web` 退出 0；catalog 为 registered commands:328 / unclassified:0；git diff --check 通过。
- 保留原授权安全测试，新增筛选全选、固定项目集合、折叠草稿保留、PublicTunnel 快捷操作门禁、只读降权、全权限确认、首次授权全13操作、原受限配置不扩大及真实翻译 Provider 不额外 GET 的回归。
- 独立审查发现只读预设若使用 union 会保留已有高风险权限，已改为替换六只读操作，新增从13权限降权的测试，复核 approved。
- 使用48项目/4连接的隔离 fixture，未接用户配置。桌面真实浏览器测得列表高206px、双列，四张关闭卡片 display:none；390px iframe 媒体查询下单列，document.scrollWidth=390，无横向溢出。
- Ego 独立浏览器验证：按「平台管理」筛选后批量选择8项目，SSH授权和未保存草稿保留，只读预设为6项；未确认全权限前保持6项，全局开关未被隐式启用。用户追加全权限初始行为后由上述最终回归重新验证。

## 残留风险

- 未在真实账户调用 SSH、保存生产授权或操作 PTY。浏览器交互均为隔离 fixture，未发布/重装，因此已安装 v0.15.33 不会立即获得此次 UI。
- OpenCLI 曾出现旧会话点击位置偏差，且 `keys Space` 未生成有效空格事件。使用真实 i18next 测试、独立 Ego 与新 Chrome 会话排除了普通编辑导致翻译函数变化/额外重载的假设，没有据此修改产品状态逻辑。
- 命令黑名单是后续独立方案研究 req-8f01105f55c188c39fdf0e9609e35343，本轮尚未实现。默认全权限不等于命令沙箱，也不会自动设置无限文件根目录。
- 用户原有 .gitignore 与 scripts/release/__pycache__ 保持不动。
