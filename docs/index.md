# Se Manager Documentation Index

**Type:** monolith
**Primary Language:** TypeScript and Rust
**Architecture:** Tauri desktop application with React renderer and Rust native runtime
**Last Updated:** 2026-05-09

## Project Overview

Se Manager is a project-aware desktop terminal workspace built with Tauri 2. It combines terminal management, project organization, snapshots, file exploration, editor panels, embedded browser tabs, and browser annotation workflows inside a single desktop shell.

## Quick Reference

- **Tech Stack:** Tauri 2, Rust, React 18, TypeScript, Zustand, xterm.js, Tailwind CSS, Radix UI/shadcn
- **Entry Point:** `src/renderer/tauri-main.tsx` / `src-tauri/src/main.rs`
- **Architecture Pattern:** Layered desktop application with renderer/runtime separation
- **Repository Type:** Monolith
- **Deployment:** GitHub Actions release pipeline with signed updater artifacts

## Generated Documentation

### Core Documentation

- [Legacy Project Context](./project-context.md) - Historical AI-context snapshot; reference only, not current instructions
- [Project Overview](./project-overview.md) - Executive summary and high-level architecture
- [Architecture](./architecture.md) - Detailed technical architecture
- [Source Tree Analysis](./source-tree-analysis.md) - Annotated directory structure
- [Component Inventory](./component-inventory.md) - Catalog of major UI and workspace components
- [会话列表与终端列表盘点](./list-ui-inventory.md) - 会话、CLI 会话、终端列表的现状盘点与已锁定的实现顺序
- [优化借鉴目录](./borrowing/README.md) - 外部参考项目(grok-bot / ThinkRail / NyaTerm)的对抗性评估与借鉴方案总入口,含合并优先级视图
  - [grok-bot 借鉴报告与方案](./borrowing/grok-bot-borrowing-plan.md) - 用量仪表盘(ACP+终端 CLI+定价匹配)、MCP 反向桥、composing 脉冲的调研证据与分阶段方案
  - [ThinkRail 借鉴报告](./borrowing/thinkrail-borrowing-plan.md) - JetBrains/thinkrail 参考仓库的 Composer 三态发送、终端连接语义、Review 锚定、gh PR 流程等借鉴项
  - [终端渲染层借鉴: WebGL vs DOM](./borrowing/terminal-renderer-dom-fallback.md) - ThinkRail 弃用 WebglAddon 的决策证据、崩溃机制对比、Se 回退路径现状与 A/B 实验设计
  - [NyaTerm 借鉴报告](./borrowing/nyaterm-borrowing-plan.md) - 同栈运维终端的 SSH 远程系统监控采样器模式、主题导入导出/自定义主题/终端主题独立等借鉴项
- [Obsidian 式文档能力可行性](./borrowing/obsidian-doc-capability-feasibility.md) - vault/wiki 链接/backlinks + WebDAV 同步(reqwest_dav)+ 本地向量化(sqlite-vec+fastembed)的实现路径与可借鉴项目清单
- [Development Guide](./development-guide.md) - Local setup, commands, and developer workflows
- [API Contracts](./api-contracts.md) - Internal Tauri IPC command/event contracts
- [Terminal Runtime Evaluation](./terminal-runtime-evaluation.md) - Terminal rendering audit and tmux/RMUX replacement analysis
- [Leftover / stacked glyphs handoff](./leftover-glyphs-handoff.md) - Unsolved zsh/WebGL leftover investigation, failed attempts, and next experiments
- [Deployment Guide](./deployment-guide.md) - Release, packaging, and updater workflow
- [Contribution Guide](./contribution-guide.md) - Contribution process and coding conventions

### Optional / Conditional Documentation

- [Data Models](./data-models.md) _(To be generated)_

## Existing Documentation

- [README](../README.md) - User-facing overview, installation, usage, and tech stack summary
- [Contributing](../CONTRIBUTING.md) - Contributor workflow and maintainer release notes
- [Auto Update Release Verification](./auto-update-release-verification.md) - Operational updater and release verification notes
- [Project Scan Analysis](./project-scan-analysis.json) - Prior scan artifact summarizing repository composition
- [PR Template](../.github/PULL_REQUEST_TEMPLATE.md) - Pull request guidance
- [PR Validation Workflow](../.github/workflows/pr-validation.yml) - CI validation rules for pull requests
- [Release Workflow](../.github/workflows/release.yml) - Release build and publish automation
- [Publish AUR Workflow](../.github/workflows/publish-aur.yml) - Arch Linux AUR publishing automation
- [Code Review Workflow](../.github/workflows/code-review.yml) - Automated review workflow for dev PRs
- [Fork Monitor Workflow](../.github/workflows/fork-monitor.yml) - Repository monitoring workflow

## Getting Started

### Prerequisites

- Bun 1.3+
- Rust toolchain
- Platform-specific Tauri dependencies from the README

### Setup

```bash
bun install
```

### Run Locally

```bash
bun run dev
```

### Run Tests

```bash
bun run test
```

## For AI-Assisted Development

This documentation is intended to help AI tools understand and safely extend the codebase.

**Agent instructions:** [`../AGENTS.md`](../AGENTS.md)

### When Planning New Features

- **UI-only features:** Reference `architecture.md`, `component-inventory.md`, `list-ui-inventory.md`, and `source-tree-analysis.md`
- **Runtime/native features:** Reference `architecture.md`, `api-contracts.md`, and `source-tree-analysis.md`
- **Terminal features:** Start with `architecture.md` and `api-contracts.md`
- **Browser annotation features:** Start with `architecture.md`, `component-inventory.md`, and `api-contracts.md`
- **Release/deployment changes:** Reference `deployment-guide.md` and `auto-update-release-verification.md`

---

_Documentation generated by BMAD Method `document-project` workflow_
