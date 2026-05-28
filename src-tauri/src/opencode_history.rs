use rayon::prelude::*;
use rusqlite;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::history::{
    compact_text, epoch_to_iso, extract_keywords, infer_title, HistoryResponse, Message,
    SessionSummary, ToolCall,
};

static STOP_WORDS_OPENCODE: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "you", "your", "opencode",
    "session", "message", "function", "output", "input", "text", "type", "content", "tool", "file",
    "code", "true", "false", "null", "name", "value", "一个", "这个", "那个", "我们", "你们",
    "他们", "进行", "可以", "需要", "以及", "如果", "因为", "所以", "然后", "历史", "会话", "记录",
];

pub fn load(custom_root: Option<String>) -> Result<HistoryResponse, String> {
    let root = custom_root
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_opencode_root);
    let root = root
        .canonicalize()
        .map_err(|e| format!("无法访问 OpenCode 数据目录 {}: {e}", root.display()))?;

    // 搜索 SQLite 数据库（v1.14+）
    if let Some(db_path) = find_sqlite_db(&root) {
        return load_from_sqlite(&root, &db_path);
    }

    // 回退到 JSON 文件格式（旧版）
    let storage = root.join("storage");
    if storage.exists() {
        return load_from_json(&root, &storage);
    }

    Ok(HistoryResponse {
        root: root.display().to_string(),
        sessions: Vec::new(),
        skipped: Vec::new(),
    })
}

// ── SQLite 数据库查找 ──────────────────────────────────────────────────────

fn find_sqlite_db(root: &Path) -> Option<PathBuf> {
    let candidates = [
        root.join("opencode.db"),
        root.join("storage").join("opencode.db"),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }
    find_db_recursive(root, 3)
}

fn find_db_recursive(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_db_recursive(&path, depth - 1) {
                return Some(found);
            }
        } else {
            let ext = path.extension().and_then(|v| v.to_str()).unwrap_or("");
            if ext == "db" || ext == "sqlite" || ext == "sqlite3" {
                return Some(path);
            }
        }
    }
    None
}

// ── Schema 探测 ────────────────────────────────────────────────────────────
//
// 不同版本的 OpenCode 可能使用不同的列名（snake_case / camelCase）。
// 运行时通过 PRAGMA table_info 动态探测列名，构建安全的 SQL。

struct DiscoveredSchema {
    sess_table: String,
    sess_col_id: String,
    sess_col_title: String,
    sess_col_directory: String,
    sess_col_parent: String,
    sess_col_model: String,
    sess_col_created: String,
    sess_col_updated: String,
    msg_table: String,
    msg_col_id: String,
    msg_col_session_id: String,
    msg_col_data: String,
    part_table: String,
    part_col_message_id: String,
    part_col_session_id: String,
    part_col_data: String,
    part_col_created: String,
}

impl DiscoveredSchema {
    fn discover(conn: &rusqlite::Connection) -> Result<Self, String> {
        let tables = get_table_names(conn)?;

        let sess_table = find_table(&tables, &["session", "sessions"])?;
        let msg_table = find_table(&tables, &["message", "messages"])?;
        let part_table = find_table(&tables, &["part", "parts"])?;

        let sess_cols = get_columns(conn, &sess_table)?;
        let msg_cols = get_columns(conn, &msg_table)?;
        let part_cols = get_columns(conn, &part_table)?;

        Ok(Self {
            sess_col_id: find_col(&sess_cols, &["id"])?,
            sess_col_title: find_col(&sess_cols, &["title", "name"])?,
            sess_col_directory: find_col_or(&sess_cols, &["directory", "cwd", "workdir"], ""),
            sess_col_parent: find_col_or(
                &sess_cols,
                &["parent_id", "parentSessionID", "parent"],
                "",
            ),
            sess_col_model: find_col_or(&sess_cols, &["model", "modelID"], ""),
            sess_col_created: find_col(&sess_cols, &["time_created", "createdAt", "created_at"])?,
            sess_col_updated: find_col(&sess_cols, &["time_updated", "updatedAt", "updated_at"])?,
            msg_col_id: find_col(&msg_cols, &["id"])?,
            msg_col_session_id: find_col(&msg_cols, &["session_id", "sessionId"])?,
            msg_col_data: find_col(&msg_cols, &["data", "content", "parts"])?,
            part_col_message_id: find_col(&part_cols, &["message_id", "messageId"])?,
            part_col_session_id: find_col(&part_cols, &["session_id", "sessionId"])?,
            part_col_data: find_col(&part_cols, &["data", "content"])?,
            part_col_created: find_col(&part_cols, &["time_created", "createdAt", "created_at"])?,
            sess_table,
            msg_table,
            part_table,
        })
    }

    fn session_sql(&self) -> String {
        format!(
            "SELECT {id}, {title}, {dir}, {parent}, {model}, {created}, {updated} \
             FROM {tbl} ORDER BY {updated} DESC",
            id = self.sess_col_id,
            title = self.sess_col_title,
            dir = self.sess_col_directory,
            parent = self.sess_col_parent,
            model = self.sess_col_model,
            created = self.sess_col_created,
            updated = self.sess_col_updated,
            tbl = self.sess_table,
        )
    }

    /// 全量加载所有 message（批量，避免 N+1）
    fn message_bulk_sql(&self) -> String {
        format!(
            "SELECT {sid}, {id}, {data} FROM {tbl}",
            sid = self.msg_col_session_id,
            id = self.msg_col_id,
            data = self.msg_col_data,
            tbl = self.msg_table,
        )
    }

    /// 全量加载所有 part（批量，避免 N+1）
    fn part_bulk_sql(&self) -> String {
        format!(
            "SELECT {sid}, {mid}, {data}, {created} FROM {tbl} ORDER BY {created} ASC",
            sid = self.part_col_session_id,
            mid = self.part_col_message_id,
            data = self.part_col_data,
            created = self.part_col_created,
            tbl = self.part_table,
        )
    }
}

fn get_table_names(conn: &rusqlite::Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .map_err(|e| format!("查询表列表失败: {e}"))?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| format!("读取表名失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names)
}

fn find_table<'a>(tables: &[String], candidates: &[&'a str]) -> Result<String, String> {
    for c in candidates {
        if tables.iter().any(|t| t == *c) {
            return Ok(c.to_string());
        }
    }
    Err(format!("找不到数据库表 (候选: {})", candidates.join(", ")))
}

fn get_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("查询 {table} 表结构失败: {e}"))?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("读取 {table} 列名失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(cols)
}

fn find_col(cols: &[String], candidates: &[&str]) -> Result<String, String> {
    for c in candidates {
        if cols.iter().any(|col| col == *c) {
            return Ok(c.to_string());
        }
    }
    Err(format!("找不到列 (候选: {})", candidates.join(", ")))
}

fn find_col_or(cols: &[String], candidates: &[&str], default: &str) -> String {
    find_col(cols, candidates).unwrap_or_else(|_| default.to_string())
}

// ── SQLite 加载（v1.14+）─────────────────────────────────────────────────────

fn load_from_sqlite(root: &Path, db_path: &Path) -> Result<HistoryResponse, String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("无法打开 SQLite 数据库 {}: {e}", db_path.display()))?;

    let schema = DiscoveredSchema::discover(&conn).map_err(|e| {
        format!(
            "OpenCode 数据库 {} schema 不兼容: {e}（可能需要更新应用版本）",
            db_path.display()
        )
    })?;

    // ── 批量加载 sessions ──
    let sess_sql = schema.session_sql();
    let mut stmt = conn
        .prepare(&sess_sql)
        .map_err(|e| format!("执行 session 查询失败: {e}"))?;

    let sess_rows: Vec<(String, String, String, String, String, i64, i64)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, String>(1).unwrap_or_default(),
                row.get::<_, String>(2).unwrap_or_default(),
                row.get::<_, String>(3).unwrap_or_default(),
                row.get::<_, String>(4).unwrap_or_default(),
                row.get::<_, i64>(5).unwrap_or(0),
                row.get::<_, i64>(6).unwrap_or(0),
            ))
        })
        .map_err(|e| format!("读取 session 行失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // ── 批量加载所有 messages，按 session_id 分组 ──
    let msg_sql = schema.message_bulk_sql();
    let mut msg_stmt = conn
        .prepare(&msg_sql)
        .map_err(|e| format!("执行 message 批量查询失败: {e}"))?;

    let all_msgs: Vec<(String, String, String)> = msg_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(), // session_id
                row.get::<_, String>(1).unwrap_or_default(), // id
                row.get::<_, String>(2).unwrap_or_default(), // data
            ))
        })
        .map_err(|e| format!("读取 message 行失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    drop(msg_stmt);

    let mut msg_by_session: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (sid, mid, data) in all_msgs {
        msg_by_session.entry(sid).or_default().push((mid, data));
    }

    // ── 批量加载所有 parts，按 session_id 分组 ──
    let part_sql = schema.part_bulk_sql();
    let mut part_stmt = conn
        .prepare(&part_sql)
        .map_err(|e| format!("执行 part 批量查询失败: {e}"))?;

    let all_parts: Vec<(String, String, String, i64)> = part_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(), // session_id
                row.get::<_, String>(1).unwrap_or_default(), // message_id
                row.get::<_, String>(2).unwrap_or_default(), // data
                row.get::<_, i64>(3).unwrap_or(0),           // time_created
            ))
        })
        .map_err(|e| format!("读取 part 行失败: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    drop(part_stmt);

    let mut parts_by_session: HashMap<String, Vec<(String, String, i64)>> = HashMap::new();
    for (sid, mid, data, ts) in all_parts {
        parts_by_session
            .entry(sid)
            .or_default()
            .push((mid, data, ts));
    }

    // ── 组装 sessions ──
    let mut sessions = Vec::with_capacity(sess_rows.len());

    for (sid, stitle, directory, parent_id, smodel, created_at, updated_at) in &sess_rows {
        let msgs = msg_by_session.get(sid).map(|v| v.as_slice()).unwrap_or(&[]);
        let parts = parts_by_session
            .get(sid)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        // 从 message.data 构建 id -> role 映射
        let mut msg_role_map: HashMap<String, String> = HashMap::new();
        let mut model = if smodel.is_empty() {
            String::new()
        } else {
            smodel.clone()
        };
        let mut total_tokens: Option<u64> = None;

        for (mid, data_json) in msgs {
            if let Ok(data) = serde_json::from_str::<Value>(data_json) {
                let role = data
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                msg_role_map.insert(mid.clone(), role);
                if model.is_empty() {
                    if let Some(m) = data.get("modelID").and_then(|v| v.as_str()) {
                        model = m.to_string();
                    }
                }
                if let Some(tokens) = data.get("tokens") {
                    let total = tokens.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                    if total > 0 {
                        total_tokens = Some(total_tokens.unwrap_or(0) + total);
                    }
                }
            }
        }

        let mut messages: Vec<Message> = Vec::new();
        let mut tools: Vec<ToolCall> = Vec::new();
        let mut tool_outputs = 0usize;

        // 从 part.data 提取内容
        for (msg_id, part_data_json, part_time) in parts {
            if let Ok(data) = serde_json::from_str::<Value>(part_data_json) {
                let part_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let ts = epoch_to_iso(*part_time);

                match part_type {
                    "text" => {
                        if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
                            if !text.trim().is_empty() {
                                let role = msg_role_map
                                    .get(msg_id.as_str())
                                    .map(|s| s.as_str())
                                    .unwrap_or("user");
                                messages.push(Message {
                                    role: role.to_string(),
                                    text: compact_text(text, 12_000),
                                    timestamp: ts,
                                });
                            }
                        }
                    }
                    "tool" | "tool-invocation" => {
                        let tool_name = data
                            .pointer("/tool/name")
                            .and_then(|v| v.as_str())
                            .or_else(|| data.get("toolName").and_then(|v| v.as_str()))
                            .unwrap_or("unknown");
                        let call_id = data
                            .get("callID")
                            .and_then(|v| v.as_str())
                            .or_else(|| data.get("toolCallID").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        tools.push(ToolCall {
                            name: tool_name.to_string(),
                            timestamp: ts,
                            call_id: call_id.to_string(),
                        });
                        tool_outputs += 1;
                    }
                    _ => {}
                }
            }
        }

        let title = if stitle.is_empty() {
            infer_title(&messages).unwrap_or_else(|| format!("会话 {}", &sid[..8.min(sid.len())]))
        } else {
            stitle.clone()
        };

        sessions.push(build_opencode_session(
            sid.clone(),
            title,
            db_path.display().to_string(),
            0,
            epoch_to_iso(*created_at),
            epoch_to_iso(*updated_at),
            if directory.is_empty() {
                String::new()
            } else {
                directory.clone()
            },
            model,
            parent_id.clone(),
            messages,
            tools,
            tool_outputs,
            total_tokens,
        ));
    }

    Ok(HistoryResponse {
        root: root.display().to_string(),
        sessions,
        skipped: Vec::new(),
    })
}

// ── JSON 文件加载（旧版）─────────────────────────────────────────────────────

fn load_from_json(root: &Path, storage: &Path) -> Result<HistoryResponse, String> {
    let session_dir = storage.join("session");
    let mut session_files: Vec<(PathBuf, u64)> = Vec::new();
    if session_dir.exists() {
        if let Ok(entries) = walk_dir(&session_dir, 10) {
            for path in entries {
                if path.extension().and_then(|v| v.to_str()) == Some("json") {
                    if let Ok(meta) = fs::metadata(&path) {
                        session_files.push((path, meta.len()));
                    }
                }
            }
        }
    }

    let message_base = storage.join("message");
    let mut sessions: Vec<SessionSummary> = session_files
        .par_iter()
        .filter_map(|(path, size)| parse_json_session(path, &message_base, *size).ok())
        .collect();

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let mut seen = HashSet::new();
    let sessions: Vec<SessionSummary> = sessions
        .into_iter()
        .filter(|s| seen.insert(s.id.clone()))
        .collect();

    Ok(HistoryResponse {
        root: root.display().to_string(),
        sessions,
        skipped: Vec::new(),
    })
}

fn parse_json_session(
    path: &Path,
    message_base: &Path,
    file_size: u64,
) -> Result<SessionSummary, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("读取 {}: {e}", path.display()))?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 {}: {e}", path.display()))?;

    let session_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Err("session ID 为空".to_string());
    }

    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    // 兼容多种时间字段命名
    let created_at = json_time(&value, &["createdAt", "time_created", "created_at"]);
    let updated_at = json_time(&value, &["updatedAt", "time_updated", "updated_at"]);
    let parent_id = json_str(&value, &["parentSessionID", "parent_id", "parentId"]);
    let directory = json_str(&value, &["directory", "cwd", "workdir"]);

    let msg_dir = message_base.join(&session_id);
    let mut messages: Vec<Message> = Vec::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    let mut model = String::new();
    let mut tool_outputs = 0usize;
    let mut total_tokens: Option<u64> = None;

    if msg_dir.exists() {
        let mut msg_entries: Vec<(i64, Value)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&msg_dir) {
            for entry in entries.flatten() {
                let msg_path = entry.path();
                if msg_path.extension().and_then(|v| v.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(msg_content) = fs::read_to_string(&msg_path) {
                    if let Ok(msg_value) = serde_json::from_str::<Value>(&msg_content) {
                        let ts =
                            json_time(&msg_value, &["createdAt", "time_created", "created_at"]);
                        msg_entries.push((ts, msg_value));
                    }
                }
            }
        }
        msg_entries.sort_by_key(|(ts, _)| *ts);

        for (ts_val, msg_value) in &msg_entries {
            let role = msg_value
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let ts_str = epoch_to_iso(*ts_val);

            if model.is_empty() {
                model = json_str(msg_value, &["modelID", "model"]);
                // 兼容嵌套 model 对象
                if model.is_empty() {
                    if let Some(m) = msg_value
                        .get("model")
                        .and_then(|v| v.get("modelID"))
                        .and_then(|v| v.as_str())
                    {
                        model = m.to_string();
                    }
                }
            }
            if let Some(tokens) = msg_value.get("tokens") {
                let total = tokens.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                if total > 0 {
                    total_tokens = Some(total_tokens.unwrap_or(0) + total);
                }
            }

            // parts 可能直接内嵌或通过 part 表关联
            if let Some(parts) = msg_value.get("parts").and_then(|v| v.as_array()) {
                extract_parts(
                    parts,
                    &role,
                    &ts_str,
                    &mut messages,
                    &mut tools,
                    &mut tool_outputs,
                );
            }
        }
    }

    Ok(build_opencode_session(
        session_id,
        title,
        path.display().to_string(),
        file_size,
        epoch_to_iso(created_at),
        epoch_to_iso(updated_at),
        directory,
        model,
        parent_id,
        messages,
        tools,
        tool_outputs,
        total_tokens,
    ))
}

fn build_opencode_session(
    id: String,
    title: String,
    path: String,
    size: u64,
    started_at: String,
    updated_at: String,
    cwd: String,
    model: String,
    forked_from: String,
    messages: Vec<Message>,
    tools: Vec<ToolCall>,
    tool_outputs: usize,
    total_tokens: Option<u64>,
) -> SessionSummary {
    let final_title = if title.is_empty() {
        infer_title(&messages).unwrap_or_else(|| format!("会话 {}", &id[..8.min(id.len())]))
    } else {
        title
    };

    let mut role_counts = HashMap::new();
    for m in &messages {
        *role_counts.entry(m.role.clone()).or_insert(0) += 1;
    }
    if tool_outputs > 0 {
        *role_counts.entry("tool".to_string()).or_insert(0) += tool_outputs;
    }

    let corpus = format!(
        "{}\n{}",
        &final_title,
        messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );

    SessionSummary {
        id,
        title: final_title,
        path,
        size,
        started_at,
        updated_at,
        cwd,
        model: if model.is_empty() {
            "OpenCode".to_string()
        } else {
            model
        },
        source: "opencode".to_string(),
        forked_from,
        messages,
        tools,
        role_counts,
        keywords: extract_keywords(&corpus, STOP_WORDS_OPENCODE),
        total_tokens,
        documents: Vec::new(),
    }
}

/// 从 parts 数组中提取 text 和 tool 信息，兼容多种字段命名
fn extract_parts(
    parts: &[Value],
    role: &str,
    ts: &str,
    messages: &mut Vec<Message>,
    tools: &mut Vec<ToolCall>,
    tool_outputs: &mut usize,
) {
    for part in parts {
        // part 可能是 {type, data:{...}} 或 {type, text} 两种格式
        let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match part_type {
            "text" => {
                let text = part
                    .get("text")
                    .and_then(|v| v.as_str())
                    .or_else(|| part.pointer("/data/text").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if !text.trim().is_empty() {
                    messages.push(Message {
                        role: role.to_string(),
                        text: compact_text(text, 12_000),
                        timestamp: ts.to_string(),
                    });
                }
            }
            "tool" | "tool-invocation" | "tool_call" => {
                let tool_name = part
                    .pointer("/tool/name")
                    .and_then(|v| v.as_str())
                    .or_else(|| part.pointer("/data/name").and_then(|v| v.as_str()))
                    .or_else(|| part.get("toolName").and_then(|v| v.as_str()))
                    .unwrap_or("unknown");
                let call_id = part
                    .get("callID")
                    .and_then(|v| v.as_str())
                    .or_else(|| part.pointer("/data/id").and_then(|v| v.as_str()))
                    .or_else(|| part.get("toolCallID").and_then(|v| v.as_str()))
                    .unwrap_or("");
                tools.push(ToolCall {
                    name: tool_name.to_string(),
                    timestamp: ts.to_string(),
                    call_id: call_id.to_string(),
                });
                *tool_outputs += 1;
            }
            _ => {}
        }
    }
}

/// 从 JSON 对象中按多个候选键名获取时间戳（毫秒）
fn json_time(v: &Value, keys: &[&str]) -> i64 {
    for k in keys {
        if let Some(ts) = v.get(*k).and_then(|v| v.as_i64()) {
            return ts;
        }
    }
    0
}

/// 从 JSON 对象中按多个候选键名获取字符串
fn json_str(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

// ── 通用工具函数 ────────────────────────────────────────────────────────────

fn default_opencode_root() -> PathBuf {
    if let Ok(v) = env::var("OPENCODE_DATA_DIR") {
        return PathBuf::from(v);
    }
    // XDG 规范（Linux/macOS）
    if let Ok(v) = env::var("XDG_DATA_HOME") {
        let p = PathBuf::from(v).join("opencode");
        if p.exists() {
            return p;
        }
    }
    if let Ok(v) = env::var("USERPROFILE") {
        return PathBuf::from(v)
            .join(".local")
            .join("share")
            .join("opencode");
    }
    if let Ok(v) = env::var("HOME") {
        return PathBuf::from(v)
            .join(".local")
            .join("share")
            .join("opencode");
    }
    PathBuf::from(".local/share/opencode")
}

/// 递归遍历目录，max_depth 限制递归深度防止意外深度搜索
fn walk_dir(dir: &Path, max_depth: usize) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    walk_dir_recursive(dir, &mut result, max_depth)?;
    Ok(result)
}

fn walk_dir_recursive(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), String> {
    if depth == 0 {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("读取目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取目录条目失败: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir_recursive(&path, out, depth - 1)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
