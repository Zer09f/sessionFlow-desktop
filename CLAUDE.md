# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

SessionFlow Desktop — Tauri 2 + Vue 3 + Rust 桌面应用，用于可视化本地 AI 编程助手的会话历史。支持 4 个数据源：Codex、Claude Code、OpenCode、Gemini CLI。所有 UI 文本和代码注释均为中文 (zh-CN)。

## Commands

```bash
npm run tauri:dev    # 启动完整 Tauri 应用（需要 Rust 工具链）
npm run dev          # 仅 Vite 前端（无后端，使用示例数据）
npm run tauri:build  # 构建生产桌面应用
npm run check        # 类型检查 + 构建前端 (vue-tsc --noEmit && vite build)
```

无测试框架，无 linter 配置。

## Architecture

### Frontend (Vue 3 + TypeScript + Vite)

- 单组件架构：整个 UI 在 `src/App.vue` 中（~940 行），无 router、无 store、无子组件拆分
- 状态通过 Vue `ref()`/`computed()` 管理，`useSourceState()` 工厂函数管理每个 tab 的状态
- 4 个数据源 tab，每个调用不同的 Tauri command
- 所有 CSS 在根目录 `styles.css`（~1738 行），通过 CSS 自定义属性实现 light/dark 主题

### Backend (Rust / Tauri 2)

- **入口**: `src-tauri/src/main.rs` 注册所有 Tauri commands
- **解析器**: 每个数据源有独立的 parser module：
  - `history.rs` — Codex 解析 + 共享工具函数（`epoch_to_iso`, `extract_keywords`, `compact_text`, `collect_jsonl_files`, `infer_title` 等）
  - `claude_history.rs` — 解析 `~/.claude/projects/**/*.jsonl` + `~/.claude/sessions/*.json`
  - `opencode_history.rs` — 解析 SQLite（v1.14+）或 JSON 文件（旧版），使用 schema 自动探测兼容不同 OpenCode 版本
  - `gemini_history.rs` — 解析 `~/.gemini/tmp/*/chats/session-*.json`
- **Commands**: `src-tauri/src/commands/`
  - `session.rs` — `restore_session`, `restore_via_client`, `open_path`（恢复会话 / 打开路径）
  - `sync.rs` — `codex_sync_status/backup/execute/restore/list_backups`（Codex model_provider/model 同步，含自动备份和回滚）
- 所有解析器返回统一的 `HistoryResponse` struct
- 并行处理用 `rayon`，OpenCode SQLite 用 `rusqlite`（bundled + backup feature）

### IPC

前端 `invoke<HistoryResponse>(commandName, { root })` → Rust command → 返回 `HistoryResponse`。

## Key Types

TS 侧定义在 `src/types.ts`，Rust 侧定义在 `src-tauri/src/history.rs`。两者必须保持同步：
`HistoryResponse`, `SessionSummary`/`CodexSession`, `Message`/`CodexMessage`, `ToolCall`, `Keyword`, `DocumentInfo`。

Sync 相关类型：`SyncStatus`, `SyncResult`, `BackupInfo`（定义在 `src/types.ts` 和 `src-tauri/src/commands/sync.rs`）。

## Data Source Default Paths

| Source | Env Override | Default Path |
|--------|-------------|-------------|
| Codex | `CODEX_HOME` | `~/.codex` |
| Claude | `CLAUDE_HOME` | `~/.claude` |
| OpenCode | `OPENCODE_DATA_DIR` | `~/.local/share/opencode` |
| Gemini | `GEMINI_CLI_HOME` → `~/.gemini` | `~/.gemini` |

## Prerequisites

完整开发需要：Node.js、Rust 工具链（`rustup`/`cargo`）、Windows 上需要 Visual Studio Build Tools（MSVC + Windows SDK）。前端构建不依赖 Rust，`npm run check` 可单独运行。
