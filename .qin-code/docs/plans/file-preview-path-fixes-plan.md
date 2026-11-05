---
schema: qin-doc/1
kind: plan
id: file-preview-path-fixes
title: Markdown和文件预览修复计划
date: 2026-11-05
status: draft
source: 本轮实际提交、Goal验收、构建和独立审查
updated: 2026-11-05
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_crocodile_1792734636572_27d03ccc6cf693c1","last_updated_by_session":"session_crocodile_1792734636572_27d03ccc6cf693c1","session_ids":["session_crocodile_1792734636572_27d03ccc6cf693c1"],"todo_refs":[{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"implement"},{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"investigate"},{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"verify"}],"goal_refs":[{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"goal-2d7024f9-fbf1-4486-9437-bc902ffbe878"}]}
---

## 目标
修复 Markdown 目录持续闪烁、终端文件链接误匹配或找不到实际文件、图片预览无法显示。网络参考保存在 file-preview-path-research。

## 非目标
不重设计编辑器，不扩大任意目录读取权限，不发布版本或打开 PR。不宣称 TIFF/HEIC 原生解码兼容。不改动用户已有 .gitignore 和 scripts/release/__pycache__。

## 任务
1. 已完成目录稳定性与真实文档订阅，提交 e44faa84。
2. 已完成软换行、Unicode cell 映射、所属工作区上下文和唯一精确后缀定位，提交 65e28686。
3. 已完成跨端安全图片二进制读取、加载/重试/换源/竞态、JFIF/APNG 和插件能力同步，提交 3ab25e57。
4. 独立审查发现两项目录边界问题后已补齐：同 ID 替换 DOM 增量重绑，以及仅用户拖拽/键盘持久化宽度。提交 193457e5，真实面板和真实 BlockNote 独立复核通过。
5. 实际执行在共享 main 中完成，每次显式仅提交本任务文件。初始托管 Launch 未实际启动，三项容器已暂停，未恢复，避免与有效执行重叠。初始集成目标分支未用于交付。

## 验收
- Goal 最终机械验收于 2026-11-05 14:27:53+08:00 passed，包含定向 Vitest、node/web/test TypeScript、能力清单与 desktop/standalone Rust 测试。
- 新图片 reader desktop/standalone 各 7 项，文件系统各 68 项，standalone binary 5 项通过。
- python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py：registered commands 325; unclassified 0。
- bun run build:web 和 bun run build:frontend:tauri 均通过。
- 真实浏览器探针：100 次父 render 后第二标题高亮未改变且无新增 observer；损坏图片提示/重试和切换到 220×360 PNG 均通过。探针页面及服务已关闭。
- 三条通道经过独立审查。目录追加审查已关闭两个 P2，无残留 actionable findings。
- 原生 Tauri 完整打包安装与用户失败图片本体未执行 smoke test。TIFF/TIF、HEIC/HEIF 仍准确报格式不支持，不回落到文本，并提供文件管理器定位恢复。
