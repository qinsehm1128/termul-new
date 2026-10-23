---
schema: qin-doc/1
kind: audit
id: new-project-terminal-and-gitignore-verification
title: 新建项目终端与Gitignore选项验收记录
date: 2026-10-24
status: draft
source: session_goal:passed; agent://project-template-ignore-review; agent://new-project-ui-review; agent://new-project-host-review; file:src-tauri/src/project_gitignore.rs
lang: zh-CN
---

## 结论

2026-10-24 00:35 +08:00，最终机械Goal验收通过。新建/克隆项目可在桌面直接选择existing或Ghostty VT屏幕观察引擎，store即时保存。新Git项目可编辑或清空默认忽略规则。模板及自定义规则走同一安全追加服务，不截断已有`.gitignore`。

默认兼容用户指定的`.qin-code/`、`.se-manage/`及当前brand实际目录。Web维持引擎只读，Git文件操作沿原local-only策略。没有发布、push、安装、重启或停止用户当前应用。

## 发现

### 已修复

- UI首版隐藏克隆引擎控件并遗漏克隆shell参数。现创建/克隆均显示引擎，clone保留shell第四参数、undefined第五参数及engine第六参数。
- shell异步检测触发表单重置。现仅closed→open重置，用户已编辑shell不会被异步结果覆盖。
- 旧目录探测响应污染新路径。现以epoch及已检查路径关联结果，切换路径立即使init/editor资格失效，pending结果不能用于提交。
- Windows安全打开最初为空实现。现文件与目录使用reparse-aware、无delete-sharing句柄，句柄检查拒绝reparse及硬链接。
- 并发请求读后写可重复追加。现跨完整read/check/write使用fs2 OS独占锁，新文件竞争以create_new重试打开处理。
- 完整块幂等不能是另一条规则的字符串后缀。现要求完整LF行边界，保留取反顺序。
- 无结尾换行的CRLF文件和裸CR边界已修复。原始字节始终保留。
- 补齐desktop log/server tracing开始、成功/no-op及失败/拒绝日志，不记录正文、cwd或原始错误。
- Unix追加目标使用持有目录fd的openat，防止打开目标时被替换路径重定向。Windows持有并验证保护目录句柄。

### 验证过程中纠正的问题

- 原worker使用root `npx tsc`遇到TS7不再支持baseUrl。项目正确的node/web/test配置全部通过，没有为此修改配置。
- 一个worker误用`bun test`绕过Vitest别名，0测试加载失败。已使用`bun run test -- ...`复跑通过。
- 新deferred测试曾错误返回数组而非IpcResult，导致假失败。修正为真实`{success:true,data:...}`并用act解析promise，原强断言保留。14项options测试最终全部通过且无新增act警告。
- 首次后台前端构建timeout误设1000ms。改为600000ms重跑通过，最终Goal再次执行双构建通过。
- 本次新增Rust模块顺序引起rustfmt失败，已修正，全workspace格式检查通过。

## 证据

### 最终机械验收

执行以下链并退出0，Goal状态passed，1/8轮：

```sh
git diff --check
bun run typecheck
bun run test -- src/renderer/components/NewProjectModal*.test.tsx src/renderer/stores/project-store.test.ts src/renderer/lib/project-gitignore-defaults.test.ts src/renderer/lib/__tests__/project-templates.test.ts src/renderer/lib/__tests__/git-api.web.test.ts src/renderer/lib/web-server-api.test.ts src/renderer/lib/__tests__/parity-checklist.test.ts
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib project_gitignore
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib git_ignore_rules
cargo test --manifest-path src-tauri/Cargo.toml -p se-manager --lib web::router::tests
cargo check --manifest-path src-tauri/Cargo.toml -p se-manager --bin se-server --features standalone-server
bun run build:frontend:tauri
bun run build:web
```

此外：

- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`通过。
- 55项既有项目持久化、MCP同步、secure/coalesce、renderer根组件与terminal-screen-engine回归通过。部分既有根组件测试输出非致命WebSocket诊断，断言全通过。
- 共享服务17项覆盖空/已有/CRLF/无EOL/裸CR/顺序取反/末尾完整块/字节前缀/NUL/尺寸/符号链接/硬链接/FIFO/并发/目录替换。
- HTTP handler新增6项覆盖真实文件追加与重试、空内容、public ingress拒绝、路径穿越、服务错误及外部symlink目标保护。
- Desktop/Web facade及HTTP transport两文件68项通过，parity清单155项通过。清单中已有System/Keyboard/QuickTerminal推荐测试缺失警告未由本任务产生。

### Windows分支

本机已有x86_64-pc-windows-msvc target。隔离Cargo harness直接`#[path]`导入真实`project_gitignore.rs`，仅使用仓库同版本fs2/windows-sys依赖：

```sh
cargo check --offline --manifest-path /Users/qs/.qin-code/scratch/new-project-ui-preview-A5mGvtB4/windows-service-check/Cargo.toml --target x86_64-pc-windows-msvc --tests
```

通过。它是Windows代码和测试的交叉类型检查，不是Windows运行时验证。

### 浏览器可观察行为

专用Ego Space 18仅挂载真实NewProjectModal，所有host/filesystem/git边界均mock。实测选中Ghostty、启用Git、显示三条默认忽略规则，编辑内容后观察顺序：mkdir→init→ignore→create，create第六参数为ghostty-vt。Web模式控件disabled、值existing、只读解释可见，clone入口不存在。

截图：`/Users/qs/.qin-code/scratch/new-project-ui-preview-A5mGvtB4/desktop-default-ignore.png`。最终脚本退出0并关闭专用Space，专用Vite服务已停止。没有真实文件创建或真实应用升级。

### 独立审查

- `agent://project-template-ignore-review`：批准。
- `agent://new-project-ui-review`：两项异步发现已修复，并有deferred-promise回归。
- `agent://new-project-host-review`：整改后最终approved，无新增独立正确性/安全发现。

## 残留风险

- 未在Windows或Linux真机运行本次新增服务测试，也未重新打包安装桌面应用。Windows专用分支仅交叉编译通过。
- 文件服务不是对其他已具备目录rename/link权限的同权限恶意进程的沙箱。协作调用有OS锁，已有alias拒绝，目录/目标句柄受保护，但不能宣称对操作期间任意inode改名或新建alias绝对隔离。此边界已写源代码并经reviewer接受。
- 浏览器验证使用隔离host mock，只证明真实组件交互与调用参数。真实服务与守卫由Rust测试证明，不能把mock预览称为已安装应用端到端验收。
- 预存未跟踪knowledge、reference lock、release Python cache不纳入本任务提交。
