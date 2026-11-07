---
schema: qin-doc/1
kind: audit
id: mcp-source-isolation-verification
title: Se内置MCP与外部聚合器来源隔离验收
date: 2026-11-07
status: draft
source: Confirmed user contract, implementation lanes, independent boundary analyses and passed Goal acceptance
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"33"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"34"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"35"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"36"}],"goal_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"goal-2df9ab33-939b-4a24-bc8d-b4938da7cd4a"}]}
---

## 结论

按用户确认的边界完成两个独立MCP连接，替代此前混合出口设计：

- `se-mcp` / internal：仅Se会话记忆、工作区及SSH内置能力。
- `se-mcp-aggregator` / aggregator：仅已配置的外部上游。

两个来源分别持有不可变来源core、settings、端口和token，列表与调用隔离，不是UI隐藏或grouped/direct切换。2026-11-07 **22:17:11 +08:00**，完整Goal机械验收passed。此轮未改真实运行配置、未发布或安装。

## 发现

### 实施边界

- 同一个se-mcp binary新增`--source internal|aggregator`。source与grouped/entry/direct正交，两种JSON/TOML客户端key固定且同步/取消同步互不删除对方。客户端写入采用共同锁、原子替换及原备份策略。
- 旧默认settings与无source参数收窄为internal，aggregator使用独立settings和3390端口段；internal保留3290端口段及原渠道偏移。schema1只作为internal读取，schema2明确source。独立服务不混用token，端口冲突/迁移失败保护另一来源。
- 来源从core构造、变更、snapshot投影到共同dispatch终点受约束。internal不解析上游secret、不连接上游；aggregator不实例化Se记忆或宿主provider。外部上游可与builtin同名，判定基于真实来源而不是名字黑名单。
- HTTP各mode及direct别名、entry四方法、resources/prompts、facade与control都消费纯来源core。配置重载分别投影同一全局控制文档，维护独立revision fence和last-good。
- 新增无凭据identity协议。bridge验证literal-loopback同origin、协议和source后才发bearer，每次POST、重读/401重试和autostart均验证；禁代理和重定向，响应marker不匹配fail closed。旧mixed服务缺marker不作为业务出口采用。仅internal管理生命周期允许严格识别、鉴权后的legacy shutdown替换。
- 桌面manage双服务容器；10项管理命令增加source参数，默认internal、非法值固定错误。OAuth和描述流程指向aggregator，全局配置写入刷新两来源。
- 桌面ACP及独立ACP Core均登记两个来源文件，new/replacement/load/resume按来源验证与去重，无效文件不回退缓存端点。standalone两个embedded listener共享来源感知watcher，与Web配置写入对接；第二bind失败回收第一个。shared-live复用桌面服务，不额外启动第三组。
- 原无config的env supervisor child已退役为固定退出2提示，避免保留第三个混合listener。session专属host_mcp及既有se-memory退役保持，不改变记忆数据、SSH授权、黑名单和PTY生命周期。
- UI展示“Se 内置 MCP”与“外部 MCP 聚合器”两卡、独立状态/mode/client表。模式偏好按来源持久化，旧mode只迁移internal。Web本机导出维持desktop-only，不新增Web凭据导出。

### 修复闭环

- 独立UI审查发现HTTP readiness缓存不能随停止/失败撤销、恢复后不重验，以及轮询错误不自动清除。已新增50项回归并修复，复核关闭两项问题；另一来源状态不受影响，stdio离线导出保留。
- 实测发现既有DefaultBodyLimit不能限制嵌套rmcp服务的有效JSON请求。未删除失败边界作为验收，改用实际逐chunk限额middleware；source→bearer→limit→dispatch，声明长度和无声明流式超限均413、零后端调用，错误响应仍带source/version。恰好限额的两种请求均正常执行。追加独立静态复核无新阻塞。
- 390px grid fixture发现双卡外包装被长配置撑开，增加min-w-0后main scrollWidth=390，两卡client/scrollWidth均356。
- 集成期间修复McpServices构造签名、测试fixture使用旧rmcp类型/transport/helper的问题。既有builtin权限测试改为显式internal并unwrap变更结果，保留权限、超时、取消与大响应拒绝断言。

## 证据

### 机械与构建

完整Goal依次通过：bridge crate测试；mcp_core库；domain/http/memory/process/gateway_service/source_isolation六项integration；ACP router及manager；standalone mcp_source_；remote host；既有host web/preview/capabilities/workspace/se-memory退役/SSH黑名单回归；指定前端测试、完整typecheck、Tauri/Web双构建；desktop lib/se-manager/se-mcp与headless standalone编译；catalog与diff检查。

关键实际执行结果：
- bridge：24测试通过，另有代理投毒环境13项通过。
- core：103测试通过；domain8、HTTP6、memory8、process4通过。
- service9、实际双gateway进程7、ACP router11、ACP manager42、clients8通过。
- 主会话embedded双入口/Web写入热重载拓扑1项通过；standalone身份、重复token拒绝、第二bind失败清理3项通过；桌面source默认/非法输入1项通过。
- 最终扩展前端9文件 **320项**通过，18文件Biome通过。
- 27个变更Rust文件scoped rustfmt通过，git diff --check通过。
- catalog：**327 registered commands / unclassified:0**；新增一个renderer来源模式偏好能力，前端计数39，源参数和秘密排除同步在同一提交。

### 独立分析

`split-ui-boundary-review`与`split-rust-boundary-review`完成静态源码分析。UI发现的两项问题已修复并复核关闭；Rust主体及后续body-limit补丁均无剩余阻塞。分析者未独立执行测试，运行证据来自实施者和主会话。曾有review provider认证失败，未将失败执行计为审查通过。

### 隔离可视验证

仅固定mock、虚构token和内存剪贴板。实际中文UI显示两张来源明确卡片，分别复制se-mcp/internal与se-mcp-aggregator/aggregator精确配置，无token哨兵显示。390px展开配置后无页面横向溢出。测试页面及mcp-split-preview服务已关闭，不触碰真实剪贴板、宿主配置或SSH。

## 残留风险

- 本轮源码完成不代表已安装应用已切换；未升级版本号、创建tag、部署或改真实客户端配置。升级后旧混合连接会收窄，外部上游需显式接入aggregator条目。
- 两来源是应用级服务/凭据边界，不是对同一OS用户的沙箱，也不鉴别任意外部代理背后实际转发的内容。
- 仍有既有dependency deprecated/unused及链接警告，另保留一个未使用ACP兼容helper警告；未为绿灯关闭检查。
- 没有实际SSH连接、用户会话数据查询或PTY操作。保留用户.gitignore和scripts/release/__pycache__。Web本机stdio导出仍不支持并显式禁用，未把它写成跨端可用能力。
