# SessionFlow Desktop 使用说明

## 系统简介

SessionFlow Desktop 是一款本地 AI 编程助手会话历史管理工具，支持以下三个主流工具：

| 工具 | 类型 | 数据目录 |
|---|---|---|
| **Codex** | CLI / 桌面端 | `~/.codex` |
| **Claude Code** | CLI | `~/.claude` |
| **OpenCode** | 桌面端 / CLI | `~/.local/share/opencode` |
| **Gemini CLI** | CLI | `~/.gemini` |

所有数据仅存储在本地，SessionFlow Desktop 本身不联网、不上传任何数据。

---

## 安装

前往 [GitHub Releases](https://github.com/用户名/仓库名/releases) 下载对应平台的安装包：

| 平台 | 文件 | 安装方式 |
|---|---|---|
| Windows | `SessionFlow Desktop_x64-setup.exe` | 双击安装 |
| macOS (M芯片) | `SessionFlow Desktop_aarch64.dmg` | 打开 → 拖入 Applications |
| macOS (Intel) | `SessionFlow Desktop_x64.dmg` | 打开 → 拖入 Applications |
| Linux | `.deb` 或 `.AppImage` | `sudo dpkg -i` 或直接运行 |

---

## 一、Codex

### 1.1 Codex 简介

Codex 是 OpenAI 推出的 AI 编程助手，有两种使用形态：

| 形态 | 启动方式 | 说明 |
|---|---|---|
| **Codex CLI** | 终端执行 `codex` | 命令行交互，纯文本界面 |
| **Codex Desktop** | 桌面应用图标 | 图形界面，带侧边栏会话管理 |

两者共享同一套数据目录 `~/.codex`，SessionFlow Desktop 同时支持读取两者的会话记录。

### 1.2 数据目录结构

```
~/.codex/
├── config.toml                # 配置文件（model_provider, model）
├── state_5.sqlite             # 会话数据库（threads 表）
├── session_index.jsonl         # 侧边栏索引文件
└── sessions/
    └── <项目名>/
        └── rollout-<日期>-<UUID>.jsonl   # 会话内容文件
```

| 文件 | 格式 | 内容 |
|---|---|---|
| `config.toml` | TOML | 当前 API 提供商、模型配置 |
| `state_5.sqlite` | SQLite | 会话元数据（标题、时间、提供商、模型） |
| `session_index.jsonl` | JSONL | 侧边栏显示用的会话列表索引 |
| `rollout-*.jsonl` | JSONL | 完整会话记录（消息、工具调用、文档编辑） |

Windows 路径：`C:\Users\<用户名>\.codex`
macOS/Linux 路径：`~/.codex`
环境变量覆盖：设置 `CODEX_HOME` 可自定义路径

### 1.3 查看会话历史

1. 启动 SessionFlow Desktop，左侧选择 **Codex** 标签页
2. 点击 **「获取历史记录」** 按钮
3. 应用扫描上述目录，显示所有会话

### 1.4 搜索与筛选

| 功能 | 说明 |
|---|---|
| 关键词搜索 | 搜索标题、工作目录、消息内容 |
| 时间范围 | 最近 7 天 / 30 天 / 90 天 / 全部 |
| 排序 | 按更新时间、消息数、工具调用数 |
| 文档筛选 | 筛选包含文件编辑的会话 |

### 1.5 CLI 恢复会话

在会话详情页点击 **「CLI 恢复」** 按钮，应用会打开一个新的终端窗口并执行：

```bash
codex resume <会话ID>
```

前提：系统已安装 Codex CLI 且 `codex` 命令在 PATH 中。

### 1.6 历史找回（核心功能）

> **注意：此功能目前仅支持 Codex 桌面端。Claude Code、OpenCode、Gemini CLI 暂不支持。**

#### 问题场景

当你在 Codex Desktop 中执行以下操作后，旧会话会从侧边栏消失：

- 切换 API 提供商（如 OpenAI → 其他提供商）
- 切换模型（如 gpt-4o → o3）
- 更换登录账号

旧会话的数据文件仍然存在磁盘上，但因为 `model_provider` / `model` 字段不匹配，Codex Desktop 不再显示它们。

#### 使用步骤

**第一步：加载历史并检查状态**

点击「获取历史记录」加载完成后，页面出现 **「历史找回」** 面板。点击 **「检查同步状态」**，显示当前配置和需要修复的会话数量：

```
当前提供商：openai          ← config.toml 中的值
当前模型：gpt-4o
数据库不匹配：15 / 42       ← 42 个线程中有 15 个标记不一致
文件不匹配：12              ← 12 个会话文件首行需要修改
索引缺失：3                 ← 3 个数据库记录不在索引中
```

**第二步：创建备份（建议）**

点击 **「创建备份」**，将当前状态备份到 `~/.codex/history_sync_backups/`。

**第三步：执行同步**

点击 **「执行同步」** 或在任意会话详情页点击 **「历史同步」** 按钮。

确认后自动执行三阶段修复：

| 阶段 | 操作 | 说明 |
|---|---|---|
| A | 更新数据库 | `UPDATE threads SET model_provider=当前值, model=当前值` |
| B | 更新会话文件 | 修改每个 `rollout-*.jsonl` 文件首行的 `session_meta` |
| C | 重建索引 | 合并数据库和索引，重新生成 `session_index.jsonl` |

执行前自动创建备份，确保可回退。

**第四步：验证**

打开 Codex Desktop，之前消失的会话应重新出现在侧边栏。

#### 注意事项

- Codex Desktop 运行时可能锁定数据库，应用会自动重试最多 40 次（约 10 秒）。如果仍失败，关闭 Codex Desktop 后重试
- 同步只修改 `model_provider` / `model` 字段和索引，不会修改会话消息内容
- 备份文件位于 `~/.codex/history_sync_backups/`

---

## 二、Claude Code

### 2.1 Claude Code 简介

Claude Code 是 Anthropic 推出的命令行 AI 编程助手，通过终端使用：

```bash
claude          # 启动新会话
claude --resume <会话ID>   # 恢复已有会话
```

### 2.2 数据目录结构

```
~/.claude/
├── sessions/
│   └── <会话ID>.json       # 会话元数据（时间、工作目录）
└── projects/
    └── <项目路径>/
        └── <会话ID>.jsonl   # 会话内容（消息、工具调用、Token 统计）
```

| 文件 | 格式 | 内容 |
|---|---|---|
| `sessions/*.json` | JSON | 会话 ID、起止时间、工作目录、启动方式 |
| `projects/**/*.jsonl` | JSONL | 完整对话记录（用户消息、助手回复、工具调用） |

Windows 路径：`C:\Users\<用户名>\.claude`
macOS/Linux 路径：`~/.claude`
环境变量覆盖：设置 `CLAUDE_HOME` 可自定义路径

### 2.3 查看会话历史

1. 左侧切换到 **Claude Code** 标签页
2. 点击 **「获取历史记录」**
3. 应用扫描 `~/.claude` 目录并显示所有会话

### 2.4 功能特性

SessionFlow Desktop 为 Claude Code 会话提供以下信息：

| 信息 | 来源 |
|---|---|
| 会话标题 | 自动生成标题 > AI 生成标题 > 首条用户消息 |
| 模型名称 | 助手回复中的 `message.model` 字段（如 `claude-sonnet-4-20250514`） |
| Token 统计 | 各轮对话的 `input_tokens + output_tokens` 累加 |
| 工具调用 | 代码编辑、文件操作、终端命令等 |
| 工作目录 | 会话启动时所在的项目路径 |

### 2.5 CLI 恢复会话

在会话详情页点击 **「CLI 恢复」** 按钮，执行：

```bash
claude --resume <会话ID>
```

前提：系统已安装 Claude Code CLI 且 `claude` 命令在 PATH 中。

### 2.6 客户端恢复

点击 **「客户端恢复」** 按钮，应用会打开 VS Code 并定位到会话的工作目录，方便你回到当时的项目上下文。

---

## 三、通用操作

### 3.1 自定义数据目录

如果数据不在默认路径，可以在路径输入框中填写自定义绝对路径，然后点击「获取历史记录」。

各工具默认路径：

| 工具 | 默认路径 |
|---|---|
| Codex | `~/.codex`（或 `CODEX_HOME` 环境变量） |
| Claude Code | `~/.claude`（或 `CLAUDE_HOME` 环境变量） |
| OpenCode | `~/.local/share/opencode` |
| Gemini CLI | `~/.gemini` |

### 3.2 打开项目目录

会话详情页点击 **「打开目录」** 按钮，在系统文件管理器中打开该会话的工作目录。

### 3.3 查看文档编辑

会话详情中的 **「文档编辑」** 区域列出该会话创建或修改的所有文件，点击文件路径可直接在编辑器中打开。

### 3.4 消息时间线

会话详情下方展示完整的消息时间线：

- **用户消息** — 你的输入内容
- **助手回复** — AI 的回答
- **工具调用** — AI 调用的工具（代码编辑、终端命令等）
- **系统消息** — 系统级指令

可通过筛选按钮只查看特定角色，或展开查看所有消息。

### 3.5 导出数据

点击右上角 **「导出」** 按钮，将会话摘要导出为 JSON 文件。

### 3.6 深色模式

左侧侧边栏底部点击太阳/月亮图标切换浅色/深色主题。

---

## 四、常见问题

### Q: 提示"读取失败"？

确认对应工具已安装且至少运行过一次，数据目录中存在会话文件。

### Q: macOS 提示"无法验证开发者"？

1. 右键点击应用 → 选择「打开」
2. 或在「系统设置 → 隐私与安全性」中点击「仍要打开」

### Q: 大文件被跳过？

超过 50 MB 的会话文件会被跳过并在状态栏中提示。这是为了避免内存占用过大。

### Q: 数据安全吗？

SessionFlow Desktop 纯本地运行，不联网、不上传数据。所有操作仅读取本地文件。唯一写入操作是 Codex 的"历史同步"功能，且执行前会自动创建备份。
