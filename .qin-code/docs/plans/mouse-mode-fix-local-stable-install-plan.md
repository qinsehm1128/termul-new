---
schema: qin-doc/1
kind: plan
id: mouse-mode-fix-local-stable-install
title: 鼠标模式恢复修复与本地正式版安装
date: 2026-10-24
status: draft
source: user:2026-10-24T09:55:42+08:00; user:2026-10-24T09:57:35+08:00; ask:2026-10-24T09:59:53+08:00
lang: zh-CN
---

## 目标

用户2026-10-24先要求关闭本地Se Manager并安装已完成最新版本，随后明确：先修复已经定位的拖选缺陷，不做任何Option/Alt强制选区或默认选区策略变更。

安装范围经ask明确答复：`canary版本直接杀掉,正式版也杀掉,然后只需要安装正式版`。目标为`/Applications/Se Manager.app`。保留Canary程序和数据，不安装/升级Canary。

## 非目标

- 不启用`macOptionClickForcesSelection`或`mouseEventsRequireAlt`，不改变原有鼠标交互策略。
- 不向正在运行的PTY注入控制码，不用reset掩盖现场。
- 不发布tag、push或远端Release，不删除Canary及用户数据。
- 不把修复代码测试通过当成用户那一次现场根因已证实。

## 任务

1. 修复renderer被动模式追踪：当前xterm关闭任一9/1000/1002/1003都清空唯一鼠标协议，Se必须一致。已复现1002h后1000l实际none但记录drag，恢复会重开。
2. 对同类SGR/SGR-pixel互斥编码SET/RESET与RIS全量复位做窄范围一致性处理，保持已有持久化DTO及旧标签兼容。
3. 宿主`se-pty/src/modes.rs`检查相同RESET/互斥SET是否导致回放错误，以红绿回归证明后修复，不修改“最早保留日志前状态”的正确边界语义。
4. 真实独立xterm实例测试、Rust回归、类型检查与独立审查。测试不打开活终端。提交修复。
5. 构建最新已提交代码的正式身份arm64 `.app`，使用已验证Ghostty预编译库。保留正式版产品身份，不生成/发布更新签名产物。
6. 验证产物后备份正式应用与必要持久数据，关闭两套应用，按验证的core生命周期方案使新正式宿主代码真正生效。不能仅更换GUI但继续运行旧core却宣称安装完成。
7. 原子替换正式bundle，保留可回滚旧程序。启动并核对bundle版本/架构/签名、执行文件指纹、进程路径及core版本。

### 当前环境

- 正式：`/Applications/Se Manager.app`，0.15.21，bundle ID `com.se-manager.app`，运行中。
- 用户Canary：`~/Applications/Se Manager Canary.app`，0.15.23-canary.1，bundle ID `com.se-manager.app.canary`，运行中。
- 另有`/Applications/Se Manager Canary.app` 0.12.3-canary.1，未见活进程，不覆盖。
- 当前源码基线`ce7fb89a`，包含新建项目功能与拖选研究，版本号0.15.23。
- 预存未跟踪knowledge/reference lock/release pycache不碰。

### 所有权

- 父：`src/renderer/utils/terminal-registry.ts`修复、集成、安装编排与最终验收。
- `mouse-mode-renderer-tests`：mode-tracker/registry相关测试，真实xterm回归。
- `mouse-mode-host-fix`：`src-tauri/crates/se-pty/src/modes.rs`及inline tests。
- `local-upgrade-core-lifecycle`：只读核查关闭、core版本与构建身份。

## 验收

- 原始失败序列在真实xterm与Se快照一致，恢复新实例保持none。
- 当前跟踪协议跨编号RESET、互斥SET、编码、RIS、分块序列和再启用回归通过。
- 默认鼠标策略选项保持未启用。desktop/shared-live/server/browser适用传输共用同一修复。
- 已确认两应用关闭，只有正式版更新并启动，安装包来源和指纹可追溯，新的core实际承载后端逻辑。
- 没有覆盖Canary或破坏用户数据，保留回滚路径；任何未完成GUI/平台检查准确列出。
