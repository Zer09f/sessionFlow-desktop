# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

SessionFlow Desktop — a Tauri 2 + Vue 3 + Rust desktop app for visualizing local AI coding assistant session history. Supports 4 sources: Codex, Claude Code, OpenCode, and Gemini CLI. All UI text and comments are in Chinese (zh-CN).

## Commands

```bash
npm run tauri:dev    # Run full Tauri app in dev mode (requires Rust toolchain)
npm run dev          # Vite frontend only (no backend, uses sample data)
npm run tauri:build  # Build production desktop app
npm run check        # Type-check + build frontend (vue-tsc --noEmit && vite build)
```

No test framework is configured. No linter is configured.

## Architecture

**Frontend** (Vue 3 + TypeScript + Vite):
- Single-component architecture: entire UI is `src/App.vue` (~940 lines), no router, no store, no component splitting
- State via Vue `ref()`/`computed()` with a `useSourceState()` factory for per-tab state
- 4 source tabs, each calling a different Tauri command
- All CSS in root `styles.css` (~1738 lines) with light/dark themes via CSS custom properties

**Backend** (Rust / Tauri 2):
- 5 Tauri commands registered in `src-tauri/src/main.rs`
- Each source has its own parser module: `history.rs` (Codex + shared utils), `claude_history.rs`, `opencode_history.rs`, `gemini_history.rs`
- `src-tauri/src/commands/session.rs` — `restore_session` command (launches CLI tools)
- All parsers return the same `HistoryResponse` struct, shared across frontend and backend
- Uses `rayon` for parallel JSONL/JSON parsing, `rusqlite` for OpenCode SQLite data

**IPC**: Frontend calls `invoke<HistoryResponse>(commandName, { root })` → Rust command → returns `HistoryResponse`.

## Key Types

Defined in both `src/types.ts` (TS) and `src-tauri/src/history.rs` (Rust): `HistoryResponse`, `CodexSession`, `CodexMessage`, `ToolCall`, `Keyword`, `DocumentInfo`. Keep both sides in sync when modifying.

## Prerequisites

Full dev requires: Node.js, Rust toolchain (`rustup`/`cargo`), and on Windows — Visual Studio Build Tools with MSVC + Windows SDK.
