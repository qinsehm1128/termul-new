---
schema: qin-doc/1
kind: audit
id: update-runtime-cohesion
title: 更新与运行状态界面整合验收
date: 2026-11-08
status: draft
source: Scoped implementation, independent 100-test/build verification and read-only review
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","last_updated_by_session":"session_tulip_1794019733357_8f5e0fabbf52f18c","session_ids":["session_tulip_1794019733357_8f5e0fabbf52f18c"],"todo_refs":[{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"40"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"41"},{"session_id":"session_tulip_1794019733357_8f5e0fabbf52f18c","id":"42"}],"goal_refs":[]}
---

## 结论

应用偏好设置 `/preferences` 已将更新、组件版本对比、安装重启与运行状态/各核心重启收敛到相邻的“更新与运行状态”分组。仅调整布局顺序和分组文案，保留原section id、组件与行为。主会话独立回归、类型检查及双前端构建通过，可进入用户授权的正式发布。

## 发现

- 原updates与runtime之间夹着privacy、data-migration及诊断分隔线。现在侧栏分类、搜索定义和实际JSX三处均为updates后紧接runtime，中间无其他分区或divider。
- 新增中英文“更新与运行状态”“隐私与数据”“诊断与重置”分组标题。隐私/数据分组仅在至少一个对应分区可用时显示。
- 保留updates/runtime锚点和深链，toast仍跳转`/preferences?section=updates`。
- UpdateComponentDiff及安全/强制安装确认、Core逐项重启确认、desktop-only runtime gate、原API/store/组件props和轮询逻辑不变。
- 只有AppPreferences.tsx及其测试、en/zh-CN settings.json四个产品/测试文件改动。没有新增命令、路由、持久状态或插件能力，目录无需新增条目。

## 证据

- 主会话任务`675381mzx1`退出0：AppPreferences、CoreRuntimeSettings、SettingsLayout、settings-categories、settings-search、UpdateAvailableToast、update-component-diff共7文件100测试通过。
- 新增9项实际DOM回归：桌面侧栏/内容直接兄弟节点关系、同组无隔断、中英文标题、四种宿主可见性组合、搜索索引及滚动目标、updates/runtime深链、浏览器不挂载runtime或调用Core API。
- 完整typecheck（node/web/test）、`build:frontend:tauri`、`build:web`及git diff --check通过。
- 执行器四文件Biome检查退出0；AppPreferences仍有33条标签关联类a11y warning，未将其报告为无警告。正常产品区块保持原样，辅助baseline诊断计数脚本未成功，不作为通过证据。
- 独立`update-cohesion-review`逐段检查精确四文件diff和相关安全/深链/测试清理代码，无阻塞结论。其未运行测试或应用，运行证据来自主会话。
- catalog checker为327 registered commands、unclassified:0。

## 残留风险

- 结构与交互验证来自jsdom，未进行原生窗口像素或真实滚动几何验证。
- 没有实际更新、安装、重启Core/GUI或操作用户PTY；真实安全动作未被测试执行。
- 用户.gitignore与scripts/release/__pycache__保持原状，不纳入本次提交。
- 此记录仅确认布局改动验收，正式版本发布和下载资产核验另行记录。
