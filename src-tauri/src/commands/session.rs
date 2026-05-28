use std::path::Path;

/// CLI 恢复：用命令行工具精确恢复到指定会话
#[tauri::command]
pub fn restore_session(
    cwd: String,
    id: String,
    source: Option<String>,
    path: Option<String>,
) -> Result<String, String> {
    if cwd.is_empty() && id.is_empty() {
        return Err("工作目录和会话 ID 均为空，无法恢复会话".to_string());
    }

    let source = source.unwrap_or_default();
    let has_id = !id.is_empty();
    let dir: &str = if cwd.is_empty() { "." } else { &cwd };

    let (cli, args) = build_cli_command(&source, has_id, &id);
    let full_cmd = build_command_string(cli, &args);

    if !cli_exists(cli) {
        return Err(format!(
            "未检测到 {} CLI，请先安装后再恢复会话。\n恢复命令: {}",
            cli, full_cmd
        ));
    }

    if open_in_terminal(dir, cli, &args) {
        return Ok(full_cmd);
    }

    Err(format!("启动 {} 失败，请确认 CLI 工具是否正常安装。", cli))
}

/// 客户端恢复：用桌面客户端或 VS Code 打开项目目录
#[tauri::command]
pub fn restore_via_client(cwd: String, source: Option<String>) -> Result<String, String> {
    if cwd.is_empty() {
        return Err("工作目录为空，无法通过客户端恢复。".to_string());
    }

    let source = source.unwrap_or_default();

    // 优先检测桌面客户端
    match source.as_str() {
        "opencode" => {
            if let Some(app_path) = find_opencode_desktop() {
                if launch_app(&app_path, &cwd) {
                    return Ok("OpenCode 桌面端".into());
                }
            }
        }
        _ => {}
    }

    // 回退到 VS Code
    if cli_exists("code") && open_in_terminal(&cwd, "code", &[]) {
        return Ok("VS Code".into());
    }

    Err("未检测到可用的桌面客户端或 VS Code，请先安装。".to_string())
}

/// 用系统默认程序打开文件或目录
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    if path.is_empty() {
        return Err("路径为空".to_string());
    }
    if !Path::new(&path).exists() {
        return Err(format!("路径不存在: {}", path));
    }

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&path).spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&path).spawn();

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&path).spawn();

    result.map(|_| ()).map_err(|e| format!("打开失败: {}", e))
}

// ── CLI 命令构建 ──

fn build_cli_command(source: &str, has_id: bool, id: &str) -> (&'static str, Vec<String>) {
    match source {
        "claude" if has_id => ("claude", vec!["--resume".into(), id.into()]),
        "claude" => ("claude", vec![]),
        "opencode" if has_id => ("opencode", vec!["--session".into(), id.into()]),
        "opencode" => ("opencode", vec![]),
        "gemini" if has_id => ("gemini", vec!["--resume".into(), id.into()]),
        "gemini" => ("gemini", vec![]),
        _ if has_id => ("codex", vec!["resume".into(), id.into()]),
        _ => ("codex", vec![]),
    }
}

fn build_command_string(cli: &str, args: &[String]) -> String {
    let mut parts = vec![cli.to_string()];
    for arg in args {
        if arg.contains(' ') {
            parts.push(format!("\"{}\"", arg));
        } else {
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

// ── CLI 检测与启动 ──

fn cli_exists(cli: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .arg("/c")
        .arg("where")
        .arg(cli)
        .output();

    #[cfg(not(target_os = "windows"))]
    let result = std::process::Command::new("which").arg(cli).output();

    result.map(|r| r.status.success()).unwrap_or(false)
}

fn open_in_terminal(cwd: &str, cli: &str, args: &[String]) -> bool {
    let full_cmd = build_command_string(cli, args);

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .arg("/c")
        .arg("start")
        .arg("")
        .current_dir(cwd)
        .arg("cmd")
        .arg("/k")
        .arg(&full_cmd)
        .spawn();

    #[cfg(target_os = "macos")]
    let result = {
        let escaped_cwd = cwd.replace('\'', "'\\''");
        let script = format!("cd '{}' && {}", escaped_cwd, full_cmd);
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "tell application \"Terminal\" to do script \"{}\"",
                script.replace('\\', "\\\\").replace('"', "\\\"")
            ))
            .spawn()
    };

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = {
        let escaped_cwd = cwd.replace('\'', "'\\''");
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "cd '{}' && nohup {} > /dev/null 2>&1 &",
                escaped_cwd, full_cmd
            ))
            .spawn()
    };

    result.map(|_| true).unwrap_or(false)
}

// ── 桌面客户端检测 ──

fn find_opencode_desktop() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA").ok()?;
        let candidates = [
            format!("{}\\Programs\\opencode\\OpenCode.exe", local),
            format!("{}\\Programs\\opencode\\opencode.exe", local),
            format!("{}\\Programs\\OpenCode\\OpenCode.exe", local),
            format!("{}\\OpenCode\\OpenCode.exe", local),
            format!("{}\\opencode\\opencode.exe", local),
        ];
        for path in &candidates {
            if Path::new(path).exists() {
                return Some(path.clone());
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let apps = ["/Applications/OpenCode.app".to_string()];
        for path in &apps {
            if Path::new(path).exists() {
                return Some(path.clone());
            }
        }
    }

    None
}

fn launch_app(app_path: &str, cwd: &str) -> bool {
    std::process::Command::new(app_path)
        .arg(cwd)
        .spawn()
        .map(|_| true)
        .unwrap_or(false)
}
