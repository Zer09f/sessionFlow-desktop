use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::history::{default_codex_root, parse_index, IndexEntry};

// ── Constants ──

const WRITE_LOCK_RETRY_LIMIT: u32 = 40;
const WRITE_LOCK_RETRY_DELAY_MS: u64 = 250;
const FILE_REPLACE_RETRY_LIMIT: u32 = 20;
const FILE_REPLACE_RETRY_DELAY_MS: u64 = 100;

// ── Response structs ──

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub config_provider: String,
    pub config_model: String,
    pub db_mismatched: usize,
    pub file_mismatched: usize,
    pub index_missing: usize,
    pub total_threads: usize,
    pub needs_sync: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub db_updated: usize,
    pub files_updated: usize,
    pub index_rebuilt: bool,
    pub backup_path: String,
}

#[derive(Debug, Serialize)]
pub struct BackupInfo {
    pub backup_path: String,
    pub db_backed_up: bool,
    pub index_backed_up: bool,
    pub session_meta_count: usize,
    pub timestamp: String,
}

// ── Config ──

#[derive(Deserialize)]
struct CodexConfig {
    model_provider: Option<String>,
    model: Option<String>,
}

fn read_codex_config(root: &Path) -> Result<(String, String), String> {
    let config_path = root.join("config.toml");
    let text =
        fs::read_to_string(&config_path).map_err(|e| format!("无法读取 config.toml: {e}"))?;
    let config: CodexConfig =
        toml::from_str(&text).map_err(|e| format!("无法解析 config.toml: {e}"))?;
    Ok((
        config.model_provider.unwrap_or_default(),
        config.model.unwrap_or_default(),
    ))
}

// ── Helpers ──

fn resolve_root(custom: Option<String>) -> PathBuf {
    if let Some(p) = custom {
        let p = p.trim().to_string();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    default_codex_root()
}

fn open_db_readonly(root: &Path) -> Result<Connection, String> {
    let db_path = root.join("state_5.sqlite");
    Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法打开数据库（只读）: {e}"))
}

fn open_db_readwrite(root: &Path) -> Result<Connection, String> {
    let db_path = root.join("state_5.sqlite");
    Connection::open(&db_path).map_err(|e| format!("无法打开数据库（读写）: {e}"))
}

fn get_thread_columns(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(threads)")
        .map_err(|e| format!("无法获取表结构: {e}"))?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("查询列名失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(cols)
}

fn count_mismatched(conn: &Connection, column: &str, value: &str) -> Result<usize, String> {
    let sql = format!(
        "SELECT COUNT(*) FROM threads WHERE {column} IS NULL OR {column} <> ?1"
    );
    let count: usize = conn
        .query_row(&sql, [value], |row| row.get(0))
        .map_err(|e| format!("计数查询失败: {e}"))?;
    Ok(count)
}

fn read_first_line(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("无法打开 {}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("读取首行失败: {e}"))?;
    Ok(line)
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("无法确定父目录: {}", path.display()))?;
    let mut attempts = 0;
    loop {
        let tmp_path = dir.join(format!(
            ".tmp-sync-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        {
            let mut f =
                fs::File::create(&tmp_path).map_err(|e| format!("创建临时文件失败: {e}"))?;
            f.write_all(content)
                .map_err(|e| format!("写入临时文件失败: {e}"))?;
            f.sync_all()
                .map_err(|e| format!("刷盘失败: {e}"))?;
        }
        match fs::rename(&tmp_path, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                attempts += 1;
                if attempts >= FILE_REPLACE_RETRY_LIMIT {
                    return Err(format!(
                        "文件被占用，替换失败（重试 {} 次）: {} — {e}",
                        attempts,
                        path.display()
                    ));
                }
                thread::sleep(Duration::from_millis(FILE_REPLACE_RETRY_DELAY_MS));
            }
        }
    }
}

fn collect_rollout_files(sessions_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rollout_files_recursive(&path, &mut files);
            }
        }
    }
    files.sort();
    files
}

fn collect_rollout_files_recursive(dir: &Path, output: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rollout_files_recursive(&path, output);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                    output.push(path);
                }
            }
        }
    }
}

// ── Backup ──

fn make_backup(root: &Path) -> Result<PathBuf, String> {
    let backup_dir = root.join("history_sync_backups");
    fs::create_dir_all(&backup_dir).map_err(|e| format!("创建备份目录失败: {e}"))?;

    let now = chrono_now();
    let backup_subdir = backup_dir.join(format!("backup-{now}"));
    fs::create_dir_all(&backup_subdir).map_err(|e| format!("创建备份子目录失败: {e}"))?;

    // Backup database
    let db_path = root.join("state_5.sqlite");
    if db_path.exists() {
        let backup_db = backup_subdir.join("state_5.sqlite");
        backup_database(&db_path, &backup_db)?;
    }

    // Backup session index
    let index_path = root.join("session_index.jsonl");
    if index_path.exists() {
        let backup_index = backup_subdir.join("session_index.jsonl");
        fs::copy(&index_path, &backup_index)
            .map_err(|e| format!("备份索引文件失败: {e}"))?;
    }

    // Snapshot session file first-line metadata
    let sessions_dir = root.join("sessions");
    if sessions_dir.exists() {
        let rollout_files = collect_rollout_files(&sessions_dir);
        let mut meta_entries: Vec<(String, String)> = Vec::new();
        for path in &rollout_files {
            if let Ok(first_line) = read_first_line(path) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                meta_entries.push((rel, first_line));
            }
        }
        if !meta_entries.is_empty() {
            let meta_path = backup_subdir.join("session_meta.jsonl");
            let mut f =
                fs::File::create(&meta_path).map_err(|e| format!("创建元数据文件失败: {e}"))?;
            for (rel, line) in &meta_entries {
                let entry = serde_json::json!({"path": rel, "first_line": line.trim_end()});
                writeln!(f, "{}", serde_json::to_string(&entry).unwrap_or_default())
                    .map_err(|e| format!("写入元数据失败: {e}"))?;
            }
        }
    }

    Ok(backup_subdir)
}

fn backup_database(source: &Path, target: &Path) -> Result<(), String> {
    let src_conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("打开源数据库失败: {e}"))?;
    src_conn
        .backup(rusqlite::DatabaseName::Main, target, None)
        .map_err(|e| format!("备份数据库失败: {e}"))?;
    Ok(())
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple timestamp: YYYYMMDD-HHMMSS approximation
    let secs = now.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let (y, m, d) = days_to_ymd(days + 719468);
    let h = time_of_day / 3600;
    let mi = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

fn days_to_ymd(mut total_days: u64) -> (u64, u64, u64) {
    let era = total_days / 146097;
    total_days -= era * 146097;
    let yoe = (total_days - total_days / 1460 + total_days / 36524 - total_days / 146096) / 365;
    let mut doy = total_days - (365 * yoe + yoe / 4 - yoe / 100);
    let mut year = yoe + era * 400;
    if doy >= 365 {
        year += 1;
        doy -= 365;
    }
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (year, month, day)
}

// ── Tauri Commands ──

#[tauri::command]
pub fn codex_sync_status(root: Option<String>) -> Result<SyncStatus, String> {
    let root = resolve_root(root);

    let (config_provider, config_model) = read_codex_config(&root)?;

    let db_path = root.join("state_5.sqlite");
    if !db_path.exists() {
        return Err("未找到 state_5.sqlite，请确认 Codex 数据目录正确".into());
    }

    // DB stats
    let conn = open_db_readonly(&root)?;
    let total_threads: usize = conn
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap_or(0);

    let columns = get_thread_columns(&conn).unwrap_or_default();
    let has_model_col = columns.iter().any(|c| c == "model");

    let db_mismatched = if has_model_col && !config_model.is_empty() {
        let provider_mm = count_mismatched(&conn, "model_provider", &config_provider).unwrap_or(0);
        let model_mm = count_mismatched(&conn, "model", &config_model).unwrap_or(0);
        // Use a combined query for distinct mismatched count
        conn.query_row(
            "SELECT COUNT(*) FROM threads WHERE model_provider IS NULL OR model_provider <> ?1 OR model IS NULL OR model <> ?2",
            [&config_provider, &config_model],
            |row| row.get(0),
        )
        .unwrap_or(provider_mm.max(model_mm))
    } else {
        count_mismatched(&conn, "model_provider", &config_provider).unwrap_or(0)
    };

    // Session file stats
    let sessions_dir = root.join("sessions");
    let mut file_mismatched = 0usize;
    if sessions_dir.exists() {
        let rollout_files = collect_rollout_files(&sessions_dir);
        for path in &rollout_files {
            if let Ok(first_line) = read_first_line(path) {
                if let Ok(v) = serde_json::from_str::<Value>(first_line.trim()) {
                    let payload_provider = v
                        .get("payload")
                        .and_then(|p| p.get("model_provider"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let payload_model = v
                        .get("payload")
                        .and_then(|p| p.get("model"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if payload_provider != config_provider
                        || (!config_model.is_empty() && payload_model != config_model)
                    {
                        file_mismatched += 1;
                    }
                }
            }
        }
    }

    // Index missing stats
    let index_path = root.join("session_index.jsonl");
    let index_entries = if index_path.exists() {
        parse_index(&index_path).unwrap_or_default()
    } else {
        HashMap::new()
    };

    // Get all DB thread IDs
    let has_archived = columns.iter().any(|c| c == "archived");
    let sql = if has_archived {
        "SELECT id FROM threads WHERE archived = 0 OR archived IS NULL"
    } else {
        "SELECT id FROM threads"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| format!("查询线程ID失败: {e}"))?;
    let db_ids: std::collections::HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("读取线程ID失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    let index_missing = db_ids
        .iter()
        .filter(|id| !index_entries.contains_key(*id))
        .count();

    let needs_sync = db_mismatched > 0 || file_mismatched > 0 || index_missing > 0;

    Ok(SyncStatus {
        config_provider,
        config_model,
        db_mismatched,
        file_mismatched,
        index_missing,
        total_threads,
        needs_sync,
    })
}

#[tauri::command]
pub fn codex_sync_backup(root: Option<String>) -> Result<BackupInfo, String> {
    let root = resolve_root(root);

    let timestamp = chrono_now();
    let backup_path = make_backup(&root)?;

    // Count session meta snapshots
    let meta_path = backup_path.join("session_meta.jsonl");
    let session_meta_count = if meta_path.exists() {
        fs::read_to_string(&meta_path)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    } else {
        0
    };

    Ok(BackupInfo {
        backup_path: backup_path.to_string_lossy().to_string(),
        db_backed_up: backup_path.join("state_5.sqlite").exists(),
        index_backed_up: backup_path.join("session_index.jsonl").exists(),
        session_meta_count,
        timestamp,
    })
}

#[tauri::command]
pub fn codex_sync_execute(root: Option<String>) -> Result<SyncResult, String> {
    let root = resolve_root(root);
    let (config_provider, config_model) = read_codex_config(&root)?;

    // Step 0: Auto-backup
    let backup_path = make_backup(&root)?;
    let backup_path_str = backup_path.to_string_lossy().to_string();

    // Phase A: Database sync
    let db_updated = sync_database(&root, &config_provider, &config_model)?;

    // Phase B: Session file sync
    let files_updated = sync_session_files(&root, &config_provider, &config_model)?;

    // Phase C: Index rebuild
    let index_rebuilt = rebuild_index(&root)?;

    Ok(SyncResult {
        db_updated,
        files_updated,
        index_rebuilt,
        backup_path: backup_path_str,
    })
}

#[tauri::command]
pub fn codex_sync_restore(backup_path: String, root: Option<String>) -> Result<String, String> {
    let root = resolve_root(root);
    let backup_dir = PathBuf::from(&backup_path);

    if !backup_dir.exists() {
        return Err(format!("备份目录不存在: {backup_path}"));
    }

    // Safety backup before restore
    let safety_backup = make_backup(&root)?;

    // Restore database
    let backup_db = backup_dir.join("state_5.sqlite");
    if backup_db.exists() {
        let live_db = root.join("state_5.sqlite");
        // Use rusqlite backup API to safely overwrite
        let src = Connection::open_with_flags(&backup_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("打开备份数据库失败: {e}"))?;
        src.backup(rusqlite::DatabaseName::Main, &live_db, None)
            .map_err(|e| format!("恢复数据库失败: {e}"))?;
        drop(src);

        // Checkpoint
        if let Ok(conn) = open_db_readwrite(&root) {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
        }
    }

    // Restore session index
    let backup_index = backup_dir.join("session_index.jsonl");
    if backup_index.exists() {
        let live_index = root.join("session_index.jsonl");
        fs::copy(&backup_index, &live_index)
            .map_err(|e| format!("恢复索引文件失败: {e}"))?;
    }

    // Restore session file first lines from metadata snapshot
    let meta_path = backup_dir.join("session_meta.jsonl");
    if meta_path.exists() {
        if let Ok(text) = fs::read_to_string(&meta_path) {
            for line in text.lines() {
                if let Ok(entry) = serde_json::from_str::<Value>(line) {
                    if let (Some(rel), Some(first_line)) = (
                        entry.get("path").and_then(|v| v.as_str()),
                        entry.get("first_line").and_then(|v| v.as_str()),
                    ) {
                        let file_path = root.join(rel);
                        if file_path.exists() {
                            if let Err(e) = replace_first_line(&file_path, first_line) {
                                eprintln!("恢复首行失败 {}: {e}", file_path.display());
                            }
                        }
                    }
                }
            }
        }
    }

    // Rebuild index to align everything
    rebuild_index(&root)?;

    Ok(format!(
        "已从备份恢复。安全备份位于: {}",
        safety_backup.to_string_lossy()
    ))
}

// ── Phase A: Database sync ──

fn sync_database(root: &Path, provider: &str, model: &str) -> Result<usize, String> {
    let mut attempts = 0;
    loop {
        match try_sync_database(root, provider, model) {
            Ok(count) => return Ok(count),
            Err(e) => {
                let is_busy = e.contains("locked") || e.contains("busy") || e.contains("BUSY");
                if is_busy && attempts < WRITE_LOCK_RETRY_LIMIT {
                    attempts += 1;
                    thread::sleep(Duration::from_millis(WRITE_LOCK_RETRY_DELAY_MS));
                } else {
                    return Err(e);
                }
            }
        }
    }
}

fn try_sync_database(root: &Path, provider: &str, model: &str) -> Result<usize, String> {
    let conn = open_db_readwrite(root)?;

    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("获取写锁失败: {e}"))?;

    let columns = get_thread_columns(&conn)?;
    let has_model_col = columns.iter().any(|c| c == "model");

    let affected = if has_model_col && !model.is_empty() {
        conn.execute(
            "UPDATE threads SET model_provider = ?1, model = ?2 WHERE model_provider <> ?1 OR model <> ?2",
            [provider, model],
        )
        .map_err(|e| format!("更新数据库失败: {e}"))?
    } else {
        conn.execute(
            "UPDATE threads SET model_provider = ?1 WHERE model_provider <> ?1",
            [provider],
        )
        .map_err(|e| format!("更新数据库失败: {e}"))?
    };

    conn.execute_batch("COMMIT")
        .map_err(|e| format!("提交失败: {e}"))?;

    let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");

    Ok(affected)
}

// ── Phase B: Session file sync ──

fn sync_session_files(root: &Path, provider: &str, model: &str) -> Result<usize, String> {
    let sessions_dir = root.join("sessions");
    if !sessions_dir.exists() {
        return Ok(0);
    }

    let rollout_files = collect_rollout_files(&sessions_dir);
    let mut updated = 0usize;

    for path in &rollout_files {
        if let Ok(needs_update) = check_session_file_needs_update(path, provider, model) {
            if needs_update {
                if let Ok(()) = rewrite_session_first_line(path, provider, model) {
                    updated += 1;
                }
            }
        }
    }

    Ok(updated)
}

fn check_session_file_needs_update(
    path: &Path,
    provider: &str,
    model: &str,
) -> Result<bool, String> {
    let first_line = read_first_line(path)?;
    let v: Value = serde_json::from_str(first_line.trim())
        .map_err(|e| format!("解析首行JSON失败: {e}"))?;

    let payload_provider = v
        .get("payload")
        .and_then(|p| p.get("model_provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let payload_model = v
        .get("payload")
        .and_then(|p| p.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let provider_match = payload_provider == provider;
    let model_match = model.is_empty() || payload_model == model;

    Ok(!provider_match || !model_match)
}

fn rewrite_session_first_line(path: &Path, provider: &str, model: &str) -> Result<(), String> {
    // Read entire file
    let content = fs::read(path).map_err(|e| format!("读取文件失败: {e}"))?;

    // Split into first line + rest
    let (first_line_end, line_ending) = find_first_line_end(&content);
    let first_line_bytes = &content[..first_line_end];
    let rest = &content[first_line_end + line_ending.len()..];

    // Parse and modify first line
    let first_line_str = std::str::from_utf8(first_line_bytes)
        .map_err(|e| format!("首行非UTF-8: {e}"))?;
    let mut v: Value = serde_json::from_str(first_line_str.trim())
        .map_err(|e| format!("解析首行JSON失败: {e}"))?;

    if let Some(payload) = v.get_mut("payload") {
        payload["model_provider"] = Value::String(provider.to_string());
        if !model.is_empty() {
            payload["model"] = Value::String(model.to_string());
        }
    }

    let new_first_line = serde_json::to_string(&v).unwrap_or_default();

    // Reassemble: new first line + line ending + rest
    let mut output = Vec::with_capacity(new_first_line.len() + line_ending.len() + rest.len());
    output.extend_from_slice(new_first_line.as_bytes());
    output.extend_from_slice(line_ending);
    output.extend_from_slice(rest);

    atomic_write(path, &output)
}

fn find_first_line_end(content: &[u8]) -> (usize, &[u8]) {
    for (i, &b) in content.iter().enumerate() {
        if b == b'\r' {
            if content.get(i + 1) == Some(&b'\n') {
                return (i, b"\r\n");
            }
            return (i, b"\r");
        }
        if b == b'\n' {
            return (i, b"\n");
        }
    }
    (content.len(), b"")
}

// ── Phase C: Index rebuild ──

fn rebuild_index(root: &Path) -> Result<bool, String> {
    let db_path = root.join("state_5.sqlite");
    let index_path = root.join("session_index.jsonl");

    if !db_path.exists() {
        return Ok(false);
    }

    // Read existing index
    let existing = if index_path.exists() {
        parse_index(&index_path).unwrap_or_default()
    } else {
        HashMap::new()
    };

    let conn = open_db_readonly(root)?;
    let columns = get_thread_columns(&conn).unwrap_or_default();
    let has_title = columns.iter().any(|c| c == "title");
    let has_updated_at = columns.iter().any(|c| c == "updated_at");
    let has_archived = columns.iter().any(|c| c == "archived");

    // Build query
    let mut select_parts = vec!["id".to_string()];
    if has_title {
        select_parts.push("title".to_string());
    }
    if has_updated_at {
        select_parts.push("updated_at".to_string());
    }
    let sql = if has_archived {
        format!(
            "SELECT {} FROM threads WHERE archived = 0 OR archived IS NULL ORDER BY id ASC",
            select_parts.join(", ")
        )
    } else {
        format!(
            "SELECT {} FROM threads ORDER BY id ASC",
            select_parts.join(", ")
        )
    };

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("查询线程失败: {e}"))?;

    struct DbThread {
        id: String,
        title: String,
        updated_at: String,
    }

    let db_threads: Vec<DbThread> = stmt
        .query_map([], |row| {
            let id: String = row.get(0).unwrap_or_default();
            let title: String = if has_title {
                row.get(1).unwrap_or_default()
            } else {
                String::new()
            };
            let updated_at: String = if has_updated_at {
                let idx = if has_title { 2 } else { 1 };
                // Try reading as integer (Unix epoch) first, then as string
                if let Ok(ts) = row.get::<_, i64>(idx) {
                    crate::history::epoch_to_iso(ts)
                } else if let Ok(s) = row.get::<_, String>(idx) {
                    s
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            Ok(DbThread {
                id,
                title,
                updated_at,
            })
        })
        .map_err(|e| format!("读取线程失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    let db_ids: std::collections::HashSet<String> =
        db_threads.iter().map(|t| t.id.clone()).collect();

    // Merge
    let mut merged: Vec<(String, String, String)> = Vec::new(); // (id, thread_name, updated_at)

    // From DB (authoritative for updated_at)
    for t in &db_threads {
        let thread_name = if let Some(existing_entry) = existing.get(&t.id) {
            if !existing_entry.title.is_empty() {
                existing_entry.title.clone()
            } else if !t.title.is_empty() {
                t.title.clone()
            } else {
                t.id.clone()
            }
        } else if !t.title.is_empty() {
            t.title.clone()
        } else {
            t.id.clone()
        };
        merged.push((t.id.clone(), thread_name, t.updated_at.clone()));
    }

    // Index-only entries (not in DB)
    for (id, entry) in &existing {
        if !db_ids.contains(id) {
            merged.push((
                id.clone(),
                if entry.title.is_empty() {
                    id.clone()
                } else {
                    entry.title.clone()
                },
                entry.updated_at.clone(),
            ));
        }
    }

    // Sort by updated_at ascending, then id
    merged.sort_by(|a, b| {
        a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0))
    });

    // Write compact JSONL
    let mut lines = String::new();
    for (id, thread_name, updated_at) in &merged {
        let entry = serde_json::json!({
            "id": id,
            "thread_name": thread_name,
            "updated_at": updated_at,
        });
        lines.push_str(&serde_json::to_string(&entry).unwrap_or_default());
        lines.push('\n');
    }

    atomic_write(&index_path, lines.as_bytes())?;

    Ok(true)
}

// ── Restore helper ──

fn replace_first_line(path: &Path, new_first_line: &str) -> Result<(), String> {
    let content = fs::read(path).map_err(|e| format!("读取文件失败: {e}"))?;
    let (first_line_end, line_ending) = find_first_line_end(&content);
    let rest = &content[first_line_end + line_ending.len()..];

    let mut output = Vec::with_capacity(
        new_first_line.len() + line_ending.len() + rest.len(),
    );
    output.extend_from_slice(new_first_line.as_bytes());
    output.extend_from_slice(line_ending);
    output.extend_from_slice(rest);

    atomic_write(path, &output)
}

// ── List backups ──

#[tauri::command]
pub fn codex_sync_list_backups(root: Option<String>) -> Result<Vec<String>, String> {
    let root = resolve_root(root);
    let backup_dir = root.join("history_sync_backups");
    if !backup_dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    if let Ok(dir) = fs::read_dir(&backup_dir) {
        for entry in dir.flatten() {
            let path = entry.path();
            if path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("backup-"))
                    .unwrap_or(false)
            {
                entries.push(path.to_string_lossy().to_string());
            }
        }
    }

    entries.sort();
    entries.reverse(); // Most recent first
    Ok(entries)
}
