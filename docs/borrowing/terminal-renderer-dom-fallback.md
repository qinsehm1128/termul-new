# 终端渲染层借鉴:WebGL vs DOM renderer(只读分析,未开发)

**Date:** 2026-09-01
**来源:** JetBrains/thinkrail 的渲染层决策(`reference/thinkrail`,本地只读 checkout)+ termul 现状核对
**关联:** [`./leftover-glyphs-handoff.md`](../leftover-glyphs-handoff.md)(glyph 调查主线,本文不改其结论,仅提供其"next experiments"之外的备选方向);[`./thinkrail-borrowing-plan.md`](./thinkrail-borrowing-plan.md) F 项的展开

## 1. ThinkRail 的决策:不实现 WebGL,直接用 DOM renderer

ThinkRail(Worktree IDE,per-worktree 高频创建/销毁终端)**不加载** `@xterm/addon-webgl`,使用 xterm.js 默认的 DOM renderer,并把这一选择写成一级架构决策(其 `architecture.md` 决策 #11)。

**Addon 清单证据**(`apps/web/src/panels/TerminalInstance.tsx:131-146`)——只有四个非渲染类 addon:

```ts
allowProposedApi: true,
term.loadAddon(new Unicode11Addon())   // 宽字符宽度
term.loadAddon(new ClipboardAddon())   // OSC 52 剪贴板
new WebFontsAddon(false)               // web 字体
// 无 WebglAddon —— 即全部
```

其 SPEC 明文:`addon-webgl` is *not* loaded, and loading it would be a regression(`apps/web/src/panels/SPEC.md:1170`)。

**决策理由**(三条,均有出处):
1. xterm 官方维护者确认 **DOM renderer 是触控支持的前提**;
2. `WebglAddon.dispose()` **泄漏 WebGL2 context**——对 per-worktree 终端高频创建销毁是致命的;
3. iOS 的 WebGL context 数量上限会导致崩溃。

**复评条件**(写死在决策里):仅当 (a) 上游给 `libghostty-vt` 出官方 WASM/npm 分发,且 (b) `ghostty-web` 越过 0.4.0 并修好鼠标上报与 OSC 8,才重新评估。

**接受的成本**:连字(ligatures)渲染差异、`rescaleOverlappingGlyphs` 开销。配套治理:服务端输出批处理(8ms flush / 32KB maxBatch / 1MB backpressure,`packages/server/src/terminal/outputBatcher.ts:1-70`)+ 前端 60ms resize 防抖。另外其终端耦合刻意保持"约十几个 xterm API 成员、无 parser hooks/decorations/serialization",使换渲染层是单文件重写(`architecture.md` 决策 #11)。

## 2. 崩溃机制对比(回答"DOM 会不会崩")

| | WebGL 路径 | DOM 路径 |
|---|---|---|
| 依赖 | GPU context(WebGL2) | 浏览器排版引擎 |
| 失败模式 | **崩溃类**:context lost / 泄漏 / 达 iOS 上限 → 画面冻结、抛异常、需 addon 重建 | **降级类**:高吞吐时掉帧、CPU 升高,渲染不死 |
| 可否治理 | 难(recovery 逻辑只能补救不能根治泄漏) | 可(批处理、backpressure、scrollback 上限) |
| 触控/移动端 | context 上限风险 | 官方推荐路径 |

结论:切换不是"消除渲染",而是**把崩溃风险(泄漏的 GPU context)换成可控的性能上限**。

## 3. termul 现状(已核对,切 DOM 的回退路径已存在)

- `src/renderer/components/terminal/ConnectedTerminal.tsx:3` 引入 `WebglAddon`;`:588/780` 持有 `webglAddonRef` 与 `disposeWebglAddon`;`:1272-1341` 有完整 recovery(`loadWebglAddon(term, isRecovery=true)`);`:1417` 首挂;`:2158` 卸载;
- `src/renderer/hooks/use-app-settings.ts:203` 注释:**xterm 6.0 移除了 @xterm/addon-canvas,DOM 是内建 fallback**——即不加载 WebglAddon 时运行时自然落到 DOM 路径;
- 与 glyph 调查的关系:`leftover-glyphs-handoff.md` 第 4 节的多次尝试(4.4 `clearTextureAtlas`、4.5 canvas 合成层)都发生在 WebGL 路径内;第 6.1/6.2 的头号嫌疑(WebGL incremental skip / canvas 未清)本身就是 WebGL 特有机制。**DOM renderer 下这两类机制整体不存在**。

## 4. 建议实验(小成本 A/B,非全局切换)

1. 加一个渲染层开关(Settings 或环境变量),值为 `webgl`(默认,现状)与 `dom`;
2. `dom` 分支 = `ConnectedTerminal.tsx` 跳过 `loadWebglAddon`,其余代码不动(验证 `use-app-settings.ts:203` 所述 fallback 是否已干净生效);
3. 在 `leftover-glyphs-handoff.md` 记录的复现场景(17:52 类 leftover、窗口拖拽)上对比:
   - glyph 缺陷是否消失;
   - 吞吐体感(`cat` 大文件/`yes`)是否可接受;
   - 多终端 + 长会话的内存/CPU 曲线;
4. 按结果三选一:保持 WebGL(缺陷另有根因)/ 全局切 DOM / **混合策略**(WebGL 默认 + 设置可切 DOM;进阶:context-lost 时自动降级 DOM 而非现在的重载 WebGL——现有 `isRecovery` 挂点可直接复用)。

## 5. 风险与边界

- DOM renderer 的性能上限是真实代价,桌面端高吞吐场景体感会差于 WebGL——所以走 A/B 而非直接切换;
- ThinkRail 加了 `WebFontsAddon` 与对比度处理(`terminalContrast.ts`),若切 DOM 需检查 termul 的字体加载与 ANSI 主题变量在 DOM 路径下渲染一致;
- 本文档为借鉴分析,不含任何代码改动;实验设计见第 4 节,待用户批准后另行实施。
