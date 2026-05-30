# SessionFlow Desktop 打包发布流程

## 概述

通过 **GitHub Actions** 实现自动化打包发布。只需推送 git tag，云端自动为 Windows / macOS / Linux 三平台构建安装包并上传至 GitHub Releases。

用户下载安装包后即可直接安装使用，无需配置任何开发环境。

---

## 一、前置条件

### 1. 代码已推送到 GitHub 仓库

确保项目已在 GitHub 上创建仓库，且本地代码已推送：

```bash
git remote -v
# 应显示 origin 指向 github.com 的仓库地址
```

如果没有：
```bash
git remote add origin https://github.com/<用户名>/<仓库名>.git
git push -u origin master
```

### 2. 确认工作流文件存在

文件 `.github/workflows/release.yml` 应已存在于项目根目录。如果尚未创建，先提交该文件：

```bash
git add .github/workflows/release.yml
git commit -m "ci: 添加自动发布工作流"
git push origin master
```

---

## 二、发布流程（3 步）

### 步骤 1：确认代码已全部提交

```bash
git status
# 确认输出 "nothing to commit, working tree clean"
```

如果有未提交的改动：
```bash
git add .
git commit -m "你的提交信息"
git push origin master
```

### 步骤 2：打版本标签

版本号格式为 `vX.Y.Z`（语义化版本）：

```bash
# 首次发布
git tag v0.1.0

# 后续版本
git tag v0.2.0
git tag v1.0.0
```

### 步骤 3：推送标签，触发自动构建

```bash
git push origin v0.1.0
```

推送 tag 后，GitHub Actions 自动启动构建任务。

---

## 三、查看构建进度

### 方式 1：GitHub 网页

1. 打开仓库页面
2. 点击顶部 **Actions** 标签
3. 左侧选 **Release** 工作流
4. 查看当前运行的构建任务

### 方式 2：命令行（需安装 gh CLI）

```bash
gh run list --workflow=release.yml
gh run watch
```

---

## 四、构建产物

构建完成后，GitHub Actions 会自动创建一个 **Draft（草稿）** Release。

### 查看草稿

1. 打开仓库页面
2. 右侧边栏点击 **Releases**
3. 看到 Draft 标记的发布

### 各平台安装包

| 平台 | 文件名示例 | 说明 |
|---|---|---|
| Windows | `Codex History Visualizer_0.1.0_x64-setup.exe` | NSIS 安装器，双击运行 |
| Windows | `Codex History Visualizer_0.1.0_x64_en-US.msi` | MSI 安装包 |
| macOS (Apple Silicon) | `Codex History Visualizer_0.1.0_aarch64.dmg` | M1/M2/M3/M4 芯片 |
| macOS (Intel) | `Codex History Visualizer_0.1.0_x64.dmg` | Intel 芯片 |
| Linux | `codex-history-visualizer_0.1.0_amd64.deb` | Debian/Ubuntu 系 |
| Linux | `codex-history-visualizer_0.1.0_amd64.AppImage` | 通用，chmod +x 后直接运行 |

### 发布草稿

确认所有平台的安装包都已上传后，点击 **Publish release** 按钮正式发布。

---

## 五、本地构建测试（可选）

在推送 tag 前，可以在本地先构建验证：

### Windows 本地构建

```bash
npm install
npm run tauri:build
```

构建产物位于：
```
src-tauri/target/release/bundle/
├── msi/          → .msi 安装包
└── nsis/         → .exe 安装包
```

### macOS 本地构建

```bash
npm install
npm run tauri:build
```

构建产物位于：
```
src-tauri/target/release/bundle/
├── dmg/          → .dmg 安装包
└── macos/        → .app 应用
```

### Linux 本地构建

需先安装系统依赖：
```bash
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
npm install
npm run tauri:build
```

构建产物位于：
```
src-tauri/target/release/bundle/
├── deb/          → .deb 包
└── appimage/     → .AppImage
```

---

## 六、版本号管理

每次发布新版本时，需要同步更新版本号：

### 1. 修改 `src-tauri/tauri.conf.json`

```json
{
  "version": "0.2.0"
}
```

### 2. 修改 `package.json`

```json
{
  "version": "0.2.0"
}
```

### 3. 提交并打新 tag

```bash
git add src-tauri/tauri.conf.json package.json
git commit -m "chore: bump version to 0.2.0"
git tag v0.2.0
git push origin master --tags
```

---

## 七、常见问题

### Q: 构建失败，提示 Rust 编译错误？

检查本地是否能通过编译：
```bash
cd src-tauri && cargo check
```

### Q: Windows 构建缺少 WebView2？

Win10 (1803+) 和 Win11 已内置 WebView2，无需额外操作。旧版 Windows 用户首次运行时系统会自动提示安装。

### Q: macOS 安装包提示"无法验证开发者"？

这是 Apple 对未签名应用的限制。用户需要：
1. 右键点击应用 → 选择"打开"
2. 或在"系统设置 → 隐私与安全性"中点击"仍要打开"

> 正式发布建议申请 Apple Developer 账号（$99/年）进行代码签名，可消除此提示。

### Q: 如何撤销错误 tag？

```bash
# 删除本地 tag
git tag -d v0.1.0

# 删除远程 tag
git push origin :refs/tags/v0.1.0
```

### Q: 构建时间多久？

- 首次构建约 10-15 分钟（需下载 Rust 依赖并编译）
- 后续构建约 5-8 分钟（有缓存）
- 三平台并行构建，总等待时间取决于最慢的平台

---

## 八、完整操作速查

```bash
# 1. 提交所有代码
git add .
git commit -m "feat: 新功能描述"
git push origin master

# 2. 打 tag 并推送
git tag v0.1.0
git push origin v0.1.0

# 3. 等待 Actions 构建完成 → GitHub Releases 页面查看 Draft → Publish release
```
