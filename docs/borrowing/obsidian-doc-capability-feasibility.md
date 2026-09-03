# Obsidian 式文档能力可行性分析(含 WebDAV 同步与向量化)

**Date:** 2026-09-01
**性质:** 可行性调研 + 可借鉴项目清单,未开发
**结论:** **能实现,且 termul 地基比预期好**——编辑器(CodeMirror 6 + `@codemirror/lang-markdown`)、file-explorer、editor workspaces 均已存在,文档能力是在现有地基上加一层"vault 索引 + 同步 + 向量检索",不需要引入新编辑器。

## 1. 能力拆解与实现路径

| 能力 | 实现路径 | 依托的现有地基 |
|---|---|---|
| Markdown 编辑/预览 | 已有 | CodeMirror 6(`package.json:102` lang-markdown)、`hooks/use-codemirror.ts` |
| Vault(笔记库) | 复用 editor workspace 目录约定,`.md` 文件即笔记 | file-explorer、`ImportEditorWorkspacesDialog` |
| `[[wiki 链接]]` 解析与跳转 | renderer 侧 remark/unified 解析 `wikilink` 语法 → CodeMirror 点击跳转 | 现有编辑器管线 |
| 反向链接 / 关系图 | 构建期/保存期扫描 vault 生成链接索引(链接图 = 简单邻接表),UI 渲染 backlinks 面板 + 图视图(如 sigma.js/cytoscape) | 同上 |
| WebDAV 同步 | **Rust 侧 `reqwest_dav` crate**(crates.io 实测 109 万下载,async tokio+reqwest)做同步引擎;增量同步用 mtime+hash 的经典两端对账 | `src-tauri/` 现有 tokio 运行时;凭证走 keychain(`secure_storage.rs`) |
| 向量化检索 | **本地嵌入 + 向量库**:`fastembed-rs`(ONNX 本地模型,Apache-2.0)生成 chunk 向量 → `sqlite-vec`(SQLite 向量扩展,Apache-2.0,"runs anywhere")存储与 KNN 检索;笔记变更时增量重建对应 chunk | termul Rust 侧若已有 sqlite 使用则零新增运行时;检索 UI 挂进现有搜索 |

数据边界:vault 是纯 Markdown 文件(与 Obsidian 同构,用户可双向迁移);索引/向量库全部是**派生数据**,可随时重建——这条不变量决定了解耦与容错都简单。

## 2. 可借鉴项目(已用 GitHub API/crates.io 实测验证)

### 架构参考(借"组织方式",优先 MIT/Apache)

| 项目 | Stars | License | 借什么 |
|---|---|---|---|
| **SilverBullet** (`SilverBulletMD/silverbullet`) | 5.9k | **MIT** | **头号参考**。纯 Markdown 文件 + 索引层的 web 笔记平台,`[[链接]]`/backlinks/命令面板的实现方式与 termul 的"文件即真相"哲学一致,TS 同栈 |
| **Foam** (`foambubble/foam`) | 17.4k | 宽松(VSCode 扩展) | `[[wikilink]]` 工作区的轻量实现:链接解析、占位笔记创建、backlinks 作为 VSCode 扩展的做法最接近"termul 编辑器加插件"形态 |
| **Joplin** (`laurent22/joplin`) | 56.2k | AGPL | **只借鉴同步架构思路**(多同步目标含 WebDAV、增量对账、E2EE 分层),不借代码(AGPL) |
| **Smart Connections** (`brianpetro/obsidian-smart-connections`) | 5.4k | MIT | **向量化 UX 参考**:嵌入 vault、写作时推荐相关笔记(而非只在搜索框)——"链接构建副驾"的产品形态值得抄 |

### 仅借设计思想(license 不兼容,禁止复制代码)

- **SiYuan**(`siyuan-note/siyuan`,46k,AGPL-3.0):块级引用、SQL 查询层、WebDAV/S3 同步的产品形态;
- **Logseq**(`logseq/logseq`,44.7k,Clojure,AGPL-3.0):outliner + 图谱的产品面。

### 组件选型(全部 MIT/Apache,实测存在且活跃)

| 组件 | 选用 | 备注 |
|---|---|---|
| WebDAV 客户端 | `reqwest_dav` 0.3.3(109 万下载) | Rust async,与 Tauri 后端同运行时;备选 npm `webdav` 5.10(若走 renderer 侧) |
| 向量存储 | `sqlite-vec`(8k stars) | SQLite 扩展,嵌入式、零服务;LanceDB(11.3k)是更重的备选,单机笔记量级用 sqlite-vec 足够 |
| 本地嵌入 | `fastembed-rs`(1k stars,ONNX) | 离线、免 API key;多语言模型选 bge-m3 类;备选:调用户已配的 provider embedding API |

## 3. 关键风险

- **license 红线**:SiYuan/Logseq/Joplin 均为 AGPL 系,只看设计不抄代码;可安全借代码的是 SilverBullet/Foam/sqlite-vec/fastembed-rs/reqwest_dav(MIT/Apache);
- **同步冲突**:WebDAV 无服务端逻辑,冲突只能客户端解决——Joplin 模式(文件级 last-writer-wins + 冲突副本)最务实,不要发明分布式算法;
- **嵌入模型体积**:本地 ONNX 模型几十~几百 MB,需懒下载 + 明确提示;或默认走 provider API、本地模型为可选;
- **vault 规模**:万级笔记的链接索引与向量重建需增量策略(按文件 mtime 变更增量处理),全量重建只留作 repair 手段;
- **范围控制**:V1 建议 = vault + `[[链接]]` + backlinks + WebDAV 同步;图谱视图与向量化推荐放 V2(向量化依赖 V1 的 chunk 管线)。

## 4. 分阶段落地草案(未开发)

- **P1 vault 基座**:vault 目录绑定到 editor workspace;`[[wikilink]]` 解析 + 跳转 + 未创建笔记占位;保存期增量链接索引;backlinks 面板;
- **P2 WebDAV 同步**:Rust `reqwest_dav` 同步引擎 + 两端对账 + 冲突副本;凭证入 keychain;设置页同步状态;
- **P3 图谱视图**:邻接表索引 → 图谱面板(只读浏览+定位);
- **P4 向量化**:chunk 管线 + sqlite-vec + fastembed(或 provider embedding);入口 = 搜索面板扩展"语义搜索" + 写作时相关笔记推荐。
