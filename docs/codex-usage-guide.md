# SessionFlow Desktop 使用指南（Codex 篇）

## 简介

SessionFlow Desktop 是一款本地 AI 编程助手会话历史可视化工具。支持查看和管理 Codex、Claude Code、OpenCode、Gemini CLI 四种工具的历史会话记录。

本指南侧重于 **Codex** 相关功能，尤其是"历史找回"功能的使用。

---

## 安装

前往 [GitHub Releases](https://github.com/用户名/仓库名/releases) 下载对应平台的安装包：

| 平台 | 下载文件 | 安装方式 |
|---|---|---|
| Windows | `SessionFlow Desktop_1.0.0_x64-setup.exe` | 双击安装 |
| macOS (M芯片) | `SessionFlow Desktop_1.0.0_aarch64.dmg` | 打开 → 拖入 Applications |
| macOS (Intel) | `SessionFlow Desktop_1.0.0_x64.dmg` | 打开 → 拖入 Applications |
| Linux | `sessionflow-desktop_1.0.0_amd64.deb` | `sudo dpkg -i` 安装 |

安装后直接打开即可使用，无需任何额外配置。

---

## 基本使用

### 1. 查看 Codex 会话历史

1. 启动应用，左侧默认进入 **Codex** 标签页
2. 点击右上角 **「获取历史记录」** 按钮
3. 应用自动扫描 `~/.codex` 目录下的会话文件
4. 左侧面板显示所有会话列表，点击任意会话查看详情

### 2. 搜索和筛选

- **搜索框**：输入关键词搜索会话标题、工作目录、消息内容
- **时间范围**：下拉选择 7 天 / 30 天 / 90 天 / 全部
- **排序方式**：按更新时间、消息数、工具调用数排序
- **文档筛选**：筛选包含/不包含文档编辑的会话

### 3. 恢复会话

每个会话详情页提供两个操作：

| 按钮 | 功能 |
|---|---|
| **CLI 恢复** | 在新终端窗口中执行 `codex resume <会话ID>`，继续该会话 |
| **历史同步** | 触发 Codex 历史找回功能（详见下方） |

### 4. 自定义数据目录

默认读取 `~/.codex`，如果 Codex 数据在其他位置：
1. 在顶部路径输入框中填写自定义路径
2. 点击"获取历史记录"

---

## 核心功能：Codex 历史找回

### 问题背景

当你在 Codex Desktop 中切换了 API 提供商或模型后（例如从 OpenAI 切换到其他提供商），旧的会话记录仍然存在于磁盘上，但 **Codex 侧边栏不再显示它们**。

这是因为数据库和会话文件中的 `model_provider` / `model` 字段与当前配置不匹配。

### 功能位置

加载 Codex 历史记录后，页面中间会出现 **「历史找回」** 面板。

### 使用步骤

#### 第一步：检查状态

点击 **「检查同步状态」** 按钮，应用会读取当前配置并扫描数据库和文件，显示以下信息：

| 指标 | 含义 |
|---|---|
| 当前提供商 | `~/.codex/config.toml` 中配置的 `model_provider` |
| 当前模型 | `config.toml` 中配置的 `model` |
| 数据库不匹配 | `state_5.sqlite` 中 provider/model 与当前配置不一致的线程数 |
| 文件不匹配 | `rollout-*.jsonl` 文件首行元数据不一致的数量 |
| 索引缺失 | 数据库中存在但 `session_index.jsonl` 中缺失的条目 |

如果不匹配数大于 0，说明有会话可以找回。

#### 第二步（可选）：创建备份

点击 **「创建备份」** 按钮，将当前数据库、索引文件和会话元数据备份到 `~/.codex/history_sync_backups/` 目录。

> 建议首次使用时先创建备份。

#### 第三步：执行同步

点击 **「执行同步」** 按钮（或在会话详情页点击 **「历史同步」** 按钮）。

系统会弹出确认对话框，确认后自动执行三阶段修复：

1. **更新数据库** — 将 `state_5.sqlite` 中所有线程的 `model_provider` / `model` 更新为当前值
2. **更新会话文件** — 修改每个 `rollout-*.jsonl` 文件首行的元数据
3. **重建索引** — 合并数据库和索引信息，重新生成 `session_index.jsonl`

执行前会自动创建备份。

完成后显示结果：
```
同步完成：数据库更新 15 条，会话文件更新 12 个，索引已重建。
```

#### 第四步：验证

打开 **Codex Desktop**，检查侧边栏，之前消失的会话应该已经重新出现。

---

## 常见问题

### Q: 点击"检查同步状态"提示"未找到 state_5.sqlite"？

确认 Codex Desktop 已安装并运行过至少一次，且数据目录路径正确。默认路径：
- Windows: `C:\Users\<用户名>\.codex`
- macOS: `~/.codex`
- Linux: `~/.codex`

### Q: 同步时提示"数据库被占用"？

Codex Desktop 正在运行时可能锁定数据库。应用会自动重试最多 40 次（约 10 秒）。如果仍然失败：
1. 关闭 Codex Desktop
2. 重新点击"执行同步"
3. 同步完成后再打开 Codex Desktop

### Q: 同步后 Codex 侧边栏还是没有显示？

1. 完全退出并重新打开 Codex Desktop
2. 检查同步结果中数据库更新数是否为 0（可能本来就没有不匹配的记录）
3. 确认 `config.toml` 中的 `model_provider` 值是否正确

### Q: 如何恢复到同步前的状态？

同步操作前会自动创建备份，位于 `~/.codex/history_sync_backups/` 目录下。如需恢复，可通过备份文件夹中的文件手动替换。

### Q: 这个工具会修改我的会话内容吗？

不会。三阶段同步只修改：
- 数据库中的 `model_provider` / `model` 字段
- 会话文件**首行**的元数据（`session_meta`）
- `session_index.jsonl` 索引文件

会话内容（消息、工具调用、代码修改等）完全不会被改动。

### Q: macOS 提示"无法验证开发者"？

这是 Apple 对未签名应用的限制：
1. 右键点击应用 → 选择「打开」
2. 或在「系统设置 → 隐私与安全性」中点击「仍要打开」

---

## 数据目录结构

SessionFlow Desktop 读取的 Codex 数据结构：

```
~/.codex/
├── config.toml              # 当前配置（model_provider, model）
├── state_5.sqlite           # 会话元数据库（threads 表）
├── session_index.jsonl       # 侧边栏索引
├── sessions/                 # 会话文件目录
│   └── <project>/
│       └── rollout-<date>-<uuid>.jsonl   # 具体会话记录
└── history_sync_backups/     # 同步备份目录（由本工具创建）
    └── backup-20260531-120000/
        ├── state_5.sqlite        # 数据库备份
        ├── session_index.jsonl   # 索引备份
        └── session_meta.jsonl    # 会话文件首行元数据快照
```
