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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            load_codex_history,
            load_claude_history,
            load_opencode_history,
            load_gemini_history,
            commands::session::restore_session,
            commands::session::restore_via_client,
            commands::session::open_path
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri application");
}
