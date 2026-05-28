# Codex 历史会话可视化

一个 Rust + Vue + TypeScript 的本地桌面应用，用来读取并查看 Codex Desktop/CLI 的历史会话 JSONL。

## 技术栈

- 后端：Rust + Tauri command，负责扫描 `~/.codex/session_index.jsonl` 和 `~/.codex/sessions/**/*.jsonl`
- 前端：Vue 3 + TypeScript + Vite，负责可视化、筛选、详情展示
- 桌面端：Tauri 2

## 使用方式

```bash
npm install
npm start
```

启动后点击“获取历史记录”，应用会自动读取 `C:\Users\<你的用户名>\.codex`。如果你的 Codex 数据不在默认位置，可以在输入框里填写自定义目录。

只预览前端界面可以运行：

```bash
npm run dev
```

前端预览不会读取真实历史记录，真实读取需要通过 Tauri 桌面端调用 Rust 后端。

打包桌面应用可以运行：

```bash
npm run build
```

## 功能

- 会话总数、消息数、工具调用、活跃天数统计
- 每日活跃图、时段热力图、角色占比图
- 高频工具统计
- 会话搜索、排序、日期范围过滤
- 单个会话的时间线、工作目录、文件大小、关键词
- 一键导出当前筛选结果摘要

所有解析都在本机完成，应用不会上传你的历史记录。

## 开发校验

```bash
npm run check
```

当前机器需要安装这些桌面端构建前置项后，才能运行 `npm run tauri:dev` 或 `npm run tauri:build`：

- Rust 工具链：安装 `rustup`、`rustc`、`cargo`
- Windows C++ 构建工具：安装 Visual Studio Build Tools，并勾选 MSVC 与 Windows SDK

前端构建不依赖 Rust，`npm run check` 已可单独验证 Vue + TypeScript。
