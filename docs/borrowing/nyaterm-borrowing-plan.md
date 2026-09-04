# NyaTerm 借鉴报告(只读分析,未开发)

**Date:** 2026-09-01
**参考仓库:** `/Users/qs/project/me/termul/reference/nyaterm`(git clone @ nyakang/nyaterm,MIT,已 gitignore)
**定位:** NyaTerm 是与 termul **同栈**(Tauri 2 + React + Rust)的远程运维终端工作区(SSH/Telnet/Serial/RDP/VNC/SFTP + AI Assistant + 加密同步)。同栈意味着借鉴的适配成本最低——Rust 代码模式可直接对照。

## 1. 系统监控(termul 完全空白,价值最高的借鉴面)

### NyaTerm 的实现要点

- **纯 SSH 远程采集,零本地依赖**:不引 `sysinfo` crate,所有指标经 SSH 在目标机执行内嵌 bash 脚本采集——CPU/内存/磁盘/网络/负载(`cmd/stats.rs:325` 的 `SYSINFO_SCRIPT`)、进程表(`process.rs:22`)、NVIDIA GPU(`gpu.rs:39`,`nvidia-smi --query-gpu`)、昇腾 NPU(`ascend_npu.rs:61`,`npu-smi info`);
- **两帧差分采样器**:`RemoteStatsSampler` 在 Rust 侧按 session_id 缓存最近两帧,差值算 CPU% 与网络速率;TTL 10min(`stats.rs:80-95`);脚本内建 1s 超时与 trap 清理防阻塞(`stats.rs:333-360`);
- **轮询治理**:间隔可配(默认 3s,1–60s);renderer hook 连续失败 3 次自动清空、`warming_up` 态 1s 重试(`useRemoteStats.ts:22-175`);
- **UI**:ResourceMonitor 面板(CPU 环表 + 内存/磁盘/网络进度条)、GPU/NPU 独立面板带进程列表,Activity Bar 图标切换(`ResourceMonitor.tsx:169`、`GpuMonitor.tsx:29`);
- **明确不做**:无历史曲线存储(只留前一帧差分)、无本地(非 SSH)监控、无独立悬浮球窗口。

### 对 termul 的借鉴设计

1. **远程面照抄**:termul 已有 SSH 连接与 keychain 凭证(`secure_storage.rs`),把"SSH exec 内嵌脚本 + 两帧差分采样器(按 session 缓存)"整段模式搬来,得到每个远程主机的资源面板。脚本化采集的妙处:目标机零安装、无 agent、超时自清理;
2. **本地面补位**:NyaTerm 没做本地监控,termul 可用 `sysinfo` crate 补齐本机面板(同一 UI 组件,两个数据源后端);
3. **克制边界同样值得抄**:不做历史时序存储(与用量流水账区分)、失败三次自动清空、按连接独立采样——避免监控变成一个后台常驻负担。

## 2. 换肤(termul 已有 70%,借鉴收窄为三件)

### 现状对照

termul 已有完整主题模块:`src/renderer/lib/themes/`(bundled themes、`ansi-palette.ts`、`apply-theme-to-terminal.ts`、`derive-surfaces.ts`、`use-terminal-color-theme.ts`、`theme-picker-store.ts`)——UI 主题、终端配色、ANSI 调色板、表面色派生齐备。NyaTerm 的核心机制(CSS 变量注入 + xterm theme 赋值 + 热切换)termul 已具备。**真正缺的是以下三件**:

| # | NyaTerm 做法 | 证据 | 借鉴判定 |
|---|---|---|---|
| 1 | **自定义主题 + 文件导入导出**:主题 Designer 创建,`write_theme_file`/`read_theme_file` Tauri 命令走系统文件对话框,导入经 `normalizeImportedTheme`(去重 ID)+ `validateTheme` 校验后入 `custom_themes[]` | `settings.rs:168-172`、`ThemeDesignerDialog.tsx:189-225` | **✅ 借鉴**。termul 主题是封闭内置集;加"自定义主题数组 + 导入/导出 JSON + 校验"三件套,复用现有 `lib/themes/` 类型即可 |
| 2 | **终端主题独立于 UI 主题**:终端配色可单独设置,`terminal_theme: null` 时回退跟随当前 UI 主题 | `ThemeContext.tsx:83-95`、`useTerminalSettings.ts:246` | **✅ 借鉴**(需先核对 termul 现状是否已支持该回退语义,`use-terminal-color-theme.ts` 待细读) |
| 3 | **切终端主题时重建 xterm 纹理缓存**:`scheduleTextureRefresh` 防止配色切换后残影 | `useTerminalSettings.ts:201-217` | **✅ 直接对应 termul 的 glyph/纹理问题**——`terminal-webgl-repair.test.ts` 里已有 `clearTextureAtlas` 基建,补"主题切换 → 纹理刷新"触发点即可 |
| 4 | 多窗口各自主题(`ChildAppProvider` per-window) | `ChildAppProvider.tsx:202-206` | **⚠️ 暂缓**:termul 主窗口单主题,多窗口需求出现再议 |
| 5 | 主题市场/在线分发 | 无(他们也没有) | 不做 |

## 3. 顺带记录(未纳入借鉴主线)

- **AI Assistant 风险分级审批**:`assess_agent_command_risk` 模型自评 + 本地规则(sudo/rm/写操作)取 max,再按 ConfirmEach/Auto/Smart 决定是否弹审批(`agent.rs:720-790`)——若 termul 将来给 ACP agent 加命令审批,这是现成分级模型;
- **加密同步**:AES-256-GCM(master password 派生)快照 + GitHub Gist 存储 + `SyncPointer` 原子提交防冲突(`cloud_sync/`)——termul 的设置/会话若要跨设备同步可参考,暂无线索表明需要;
- **协议抽象**:RDP 的 `RdpEngine` trait、VNC session manager 等,termul 无对应需求,仅存档。

## 4. 明确不借鉴

- **只做 SSH 远程不做本地监控**的局限(NyaTerm 缺本地面板,termul 应两头都有);
- **无历史曲线**:NyaTerm 不存时序;若 termul 监控要历史,应复用用量流水账的 append-only JSONL 模式而非现造;
- 悬浮面板白名单机制、window transparency/毛玻璃(视觉向,与 termul 既有布局体系冲突面大,收益低)。

## 5. 优先级合并视图(三份参考报告)

| 优先级 | 项 | 来源 |
|---|---|---|
| 高 | composing 脉冲;用量仪表盘三源;Composer 三态发送 | grok-bot / ThinkRail |
| 高(本次新增) | **远程主机资源面板(SSH 采样器模式)** | NyaTerm |
| 中 | 主题导入导出 + 自定义主题;终端主题纹理刷新触发;终端连接三件套;Review 锚定;gh 一键 PR;悬空 toolCall 修复 | NyaTerm / ThinkRail |
| 低 | Default workspace 锚、external worktree、compaction 可见性、per-window 主题 | ThinkRail / NyaTerm |
