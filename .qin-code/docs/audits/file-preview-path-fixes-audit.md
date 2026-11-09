---
schema: qin-doc/1
kind: audit
id: file-preview-path-fixes
title: Markdown、文件定位和图片预览修复验收
date: 2026-11-05
status: accepted
updated: 2026-11-09
source: 本轮提交、测试输出、Goal验收和teammate独立复核
lang: zh-CN
scope: project
artifact: {"work_id":null,"created_by_session":"session_crocodile_1792734636572_27d03ccc6cf693c1","last_updated_by_session":"session_crocodile_1792734636572_27d03ccc6cf693c1","session_ids":["session_crocodile_1792734636572_27d03ccc6cf693c1"],"todo_refs":[{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"implement"},{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"investigate"},{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"verify"}],"goal_refs":[{"session_id":"session_crocodile_1792734636572_27d03ccc6cf693c1","id":"goal-2d7024f9-fbf1-4486-9437-bc902ffbe878"}]}
---

## 结论
Markdown 目录闪烁、终端链接截断/错根定位、以及常见栅格图片跨端预览链路已修复并提交。最终机械验收 passed，web/Tauri renderer 两个入口构建通过。用户预存工作区改动保留。

## 发现
- Markdown headings 数组引用变化曾反复重置首标题并重建 observer。现在按语义身份稳定 observer，以真实文档订阅更新目录，同 ID DOM 替换做增量重绑，保留有效高亮。
- TOC 程序化 setLayout 与约束重算会反写宽度。现在只有真实拖拽或分隔条键盘操作期间持久化，真实面板扩容 1000→2000 保持 280px，键盘仍可保存。
- 终端仅逐物理行识别且 UTF16 索引当 cell 坐标，路径可被软换行和中文/emoji错配。现在有界逻辑行和精确 cell 映射，并保留所属终端、会话工作区和 worktree 上下文。
- 直接解析 miss 后在允许根内有界查询唯一精确路径后缀，支持 apps/backend/docs 等层级。多候选、越界、home 根、超时或截断拒绝自动打开，不猜同名文件。
- Browser 原来把图片当文本，desktop 依赖 asset scope。现在共享受限栅格读取器，desktop data URL 与认证 web blob URL，20 MiB、格式签名和 MIME 检查，不允许 HTML/SVG 通过二进制通道执行。metadata 遵守相同 ingress/canonical boundary。
- 独立审查发现的目录两项 P2、CSP/blob、APNG、无 Path scope 的原生启动、缩写撇号和 macOS root home 漏项均已修复并回归。

## 证据
- e44faa840618d39c9a6111f9918124bb859d532f：目录主修复。
- 65e28686e4f05c6f053a2b16aa354cfaefb99856：终端/聊天链接定位。
- 3ab25e57cceced9d2e7ab7de44ab3d43baa78b65：图片读取及能力同步。
- 193457e5：目录真实 DOM/面板边界审查修复。
- Goal 于 2026-11-05 14:27:53+08:00 passed，最终相关 Vitest、TypeScript node/web/test、Rust desktop/standalone 与 capability check 均通过。能力清单 registered commands 325; unclassified 0。
- reader desktop/standalone 各 7 tests，fs 各 68 tests，standalone binary 5 tests 独立通过。
- bun run build:web、bun run build:frontend:tauri 于 14:28:49+08:00 前后完成，exit 0。
- 真实浏览器组件探针连续 100 次父 render：observer count不变，active 始终为第二标题。图片 error/retry 后切换 PNG，成功显示 220×360。截图在本会话 scratch，仅为可丢弃验证产物。
- 独立审查最终 approved，真实 BlockNote 同 ID 替换节点已 observe，observer只创建一次。真实面板 resize不写store且keyboard仍能持久化。

## 残留风险
- TIFF/TIF、HEIC/HEIF 没有引入缺少可靠资源上限保证的系统转码，仍明确 UNSUPPORTED_IMAGE_FORMAT，提供重试及原文件定位，不作为文本打开。
- 未拿到用户实际失败图片文件，不能声称该具体文件已打开。完整打包原生 Tauri/Finder smoke test未执行。
- 定向终端测试保留 8 项原有 skipped，本轮未新增跳过或弱化断言。
- 桌面 IPC 沿用 trusted-desktop 读取语义，不提供插件 granted-scope confinement，能力目录已明确记录。
- 仅剩用户预存 .gitignore 修改和 scripts/release/__pycache__ 未提交。
