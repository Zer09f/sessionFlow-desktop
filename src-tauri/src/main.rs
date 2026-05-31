// 防止 release 构建时弹出 CMD 窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude_history;
mod gemini_history;
mod history;
mod opencode_history;
mod commands;

#[tauri::command]
fn load_codex_history(root: Option<String>) -> Result<history::HistoryResponse, String> {
    history::load(root)
}

#[tauri::command]
fn load_claude_history(root: Option<String>) -> Result<history::HistoryResponse, String> {
    claude_history::load(root)
}

#[tauri::command]
fn load_opencode_history(root: Option<String>) -> Result<history::HistoryResponse, String> {
    opencode_history::load(root)
}

#[tauri::command]
fn load_gemini_history(root: Option<String>) -> Result<history::HistoryResponse, String> {
    gemini_history::load(root)
}

#[tauri::command]
fn save_export(filename: String, content: String) -> Result<String, String> {
    let export_dir = dirs::desktop_dir()
        .or_else(dirs::download_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let export_dir = export_dir.join("SessionFlow-exports");
    std::fs::create_dir_all(&export_dir)
        .map_err(|e| format!("创建导出目录失败: {e}"))?;
    let path = export_dir.join(&filename);
    std::fs::write(&path, &content)
        .map_err(|e| format!("写入文件失败: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            load_codex_history,
            load_claude_history,
            load_opencode_history,
            load_gemini_history,
            save_export,
            commands::session::restore_session,
            commands::session::restore_via_client,
            commands::session::open_path,
            commands::sync::codex_sync_status,
            commands::sync::codex_sync_backup,
            commands::sync::codex_sync_execute,
            commands::sync::codex_sync_restore,
            commands::sync::codex_sync_list_backups,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri application");
}
