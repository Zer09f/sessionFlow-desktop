use rayon::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::history::{
    collect_jsonl_files, compact_text, days_to_ymd, extract_keywords, infer_title, HistoryResponse,
    Message, SessionSummary, SkippedFile, ToolCall,
};

const MAX_JSONL_BYTES: u64 = 50 * 1024 * 1024;

static STOP_WORDS_CLAUDE: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "you", "your", "claude",
    "session", "message", "function", "output", "input", "text", "type", "content", "tool", "file",
    "code", "true", "false", "null", "name", "value", "一个", "这个", "那个", "我们", "你们",
    "他们", "进行", "可以", "需要", "以及", "如果", "因为", "所以", "然后", "历史", "会话", "记录",
];

pub fn load(custom_root: Option<String>) -> Result<HistoryResponse, String> {
    let root = custom_root
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_claude_root);
    let root = root
        .canonicalize()
        .map_err(|e| format!("无法访问 Claude Code 数据目录 {}: {e}", root.display()))?;

    let sessions_dir = root.join("sessions");
    let projects_dir = root.join("projects");

    let session_meta = if sessions_dir.exists() {
        parse_session_metas(&sessions_dir)?
    } else {
        HashMap::new()
    };

    let mut rollout_files = Vec::new();
    if projects_dir.exists() {
        collect_jsonl_files(&projects_dir, &mut rollout_files)
            .map_err(|e| format!("扫描 projects 目录失败: {e}"))?;
    }

    let (files_with_size, skipped): (Vec<_>, Vec<_>) = rollout_files
        .into_iter()
        .filter_map(|fp| {
            let meta = fs::metadata(&fp).ok()?;
            Some((fp, meta.len()))
        })
        .partition(|(_, size)| *size <= MAX_JSONL_BYTES);

    let skipped: Vec<SkippedFile> = skipped
        .iter()
        .map(|(fp, _)| SkippedFile {
            path: fp.display().to_string(),
            reason: format!("文件超过 {} MB", MAX_JSONL_BYTES / 1024 / 1024),
        })
        .collect();

    let meta_ref = &session_meta;
    let mut parsed: Vec<SessionSummary> = files_with_size
        .par_iter()
        .filter_map(|(file_path, size)| parse_session_jsonl(file_path, meta_ref, *size).ok())
        .collect();

    let mut seen = HashSet::new();
    let mut sessions = Vec::with_capacity(parsed.len());
    parsed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    for s in parsed {
        if seen.insert(s.id.clone()) {
            sessions.push(s);
        }
    }

    for (id, meta) in &session_meta {
        if seen.contains(id.as_str()) {
            continue;
        }
        sessions.push(SessionSummary {
            id: id.clone(),
            title: meta
                .title
                .clone()
                .unwrap_or_else(|| format!("会话 {}", &id[..8.min(id.len())])),
            path: "sessions/".to_string(),
            size: 0,
            started_at: meta.started_at.clone(),
            updated_at: meta.updated_at.clone(),
            cwd: meta.cwd.clone(),
            model: "Claude".to_string(),
            source: meta.entrypoint.clone().unwrap_or_default(),
            forked_from: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            role_counts: HashMap::new(),
            keywords: Vec::new(),
            total_tokens: None,
            documents: Vec::new(),
        });
    }

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    Ok(HistoryResponse {
        root: root.display().to_string(),
        sessions,
        skipped,
    })
}

struct SessionMeta {
    cwd: String,
    started_at: String,
    updated_at: String,
    title: Option<String>,
    entrypoint: Option<String>,
}

fn default_claude_root() -> PathBuf {
    if let Ok(v) = env::var("CLAUDE_HOME") {
        return PathBuf::from(v);
    }
    if let Ok(v) = env::var("USERPROFILE") {
        return PathBuf::from(v).join(".claude");
    }
    if let Ok(v) = env::var("HOME") {
        return PathBuf::from(v).join(".claude");
    }
    PathBuf::from(".claude")
}

fn parse_session_metas(dir: &Path) -> Result<HashMap<String, SessionMeta>, String> {
    let mut out = HashMap::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("读取 sessions 目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取目录条目失败: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let content =
            fs::read_to_string(&path).map_err(|e| format!("读取 {}: {e}", path.display()))?;
        let value: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let session_id = match value.get("sessionId").and_then(|v| v.as_str()) {
            Some(v) => v.to_string(),
            None => continue,
        };
        let started_at = format_ts(value.get("startedAt").and_then(|v| v.as_f64()));
        let updated_at = format_ts(value.get("updatedAt").and_then(|v| v.as_f64()));
        out.insert(
            session_id,
            SessionMeta {
                cwd: value
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                started_at,
                updated_at,
                title: None,
                entrypoint: value
                    .get("entrypoint")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            },
        );
    }
    Ok(out)
}

fn parse_session_jsonl(
    path: &Path,
    meta_map: &HashMap<String, SessionMeta>,
    file_size: u64,
) -> Result<SessionSummary, String> {
    let file = fs::File::open(path).map_err(|e| format!("无法打开 {}: {e}", path.display()))?;
    let reader = BufReader::with_capacity(256 * 1024, file);

    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    let mut session_id = filename
        .strip_suffix(".jsonl")
        .unwrap_or(filename)
        .to_string();

    let mut messages: Vec<Message> = Vec::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    let mut title: Option<String> = None;
    let mut started_at = String::new();
    let mut updated_at = String::new();
    let mut last_timestamp = String::new();
    let mut cwd = String::new();
    let mut model = String::from("Claude");
    let mut source = String::new();
    let mut tool_outputs = 0usize;
    let mut total_tokens: Option<u64> = None;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("读取失败 {}: {e}", path.display()))?;
        if line.is_empty() {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = value.get("type").and_then(|v| v.as_str());

        match event_type {
            Some("attachment")
            | Some("file-history-snapshot")
            | Some("queue-operation")
            | Some("permission-mode") => {
                if let Some(sid) = value.get("sessionId").and_then(|v| v.as_str()) {
                    session_id = sid.to_string();
                }
                continue;
            }
            _ => {}
        }

        if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
            last_timestamp = ts.to_string();
        }

        match event_type {
            Some("user") => {
                if let Some(sid) = value.get("sessionId").and_then(|v| v.as_str()) {
                    session_id = sid.to_string();
                }
                if let Some(c) = value.get("cwd").and_then(|v| v.as_str()) {
                    cwd = c.to_string();
                }
                if let Some(e) = value.get("entrypoint").and_then(|v| v.as_str()) {
                    source = e.to_string();
                }
                if let Some(msg) = value.get("message") {
                    let text = extract_claude_content(msg.get("content"));
                    if !text.trim().is_empty() && !is_internal_claude(&text) {
                        messages.push(Message {
                            role: "user".to_string(),
                            text: compact_text(&text, 12_000),
                            timestamp: last_timestamp.clone(),
                        });
                    }
                }
            }
            Some("assistant") => {
                if let Some(msg) = value.get("message") {
                    if let Some(m) = msg.get("model").and_then(|v| v.as_str()) {
                        model = m.to_string();
                    }
                    if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
                        for item in arr {
                            match item.get("type").and_then(|v| v.as_str()) {
                                Some("text") => {
                                    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                        if !text.trim().is_empty() {
                                            messages.push(Message {
                                                role: "assistant".to_string(),
                                                text: compact_text(text, 12_000),
                                                timestamp: last_timestamp.clone(),
                                            });
                                        }
                                    }
                                }
                                Some("tool_use") => {
                                    tools.push(ToolCall {
                                        name: item
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string(),
                                        timestamp: last_timestamp.clone(),
                                        call_id: item
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Some(usage) = msg.get("usage") {
                        if let Some(tokens) = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .zip(usage.get("output_tokens").and_then(|v| v.as_u64()))
                            .map(|(i, o)| i + o)
                        {
                            total_tokens = Some(total_tokens.unwrap_or(0) + tokens);
                        }
                    }
                }
            }
            Some("system") => {
                if let Some(sid) = value.get("sessionId").and_then(|v| v.as_str()) {
                    session_id = sid.to_string();
                }
                if value.get("subtype").and_then(|v| v.as_str()) == Some("tool") {
                    tool_outputs += 1;
                }
            }
            Some("ai-title") => {
                title = value
                    .get("aiTitle")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            Some("custom-title") => {
                title = value
                    .get("customTitle")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            _ => {}
        }
    }

    let meta = meta_map.get(&session_id);
    if started_at.is_empty() {
        started_at = meta
            .as_ref()
            .map(|m| m.started_at.clone())
            .unwrap_or_default();
    }
    if updated_at.is_empty() {
        updated_at = meta
            .as_ref()
            .map(|m| m.updated_at.clone())
            .unwrap_or_default();
    }
    if updated_at.is_empty() {
        updated_at = last_timestamp;
    }
    if started_at.is_empty() {
        started_at = updated_at.clone();
    }
    if cwd.is_empty() {
        cwd = meta.as_ref().map(|m| m.cwd.clone()).unwrap_or_default();
    }
    if source.is_empty() {
        source = meta
            .as_ref()
            .and_then(|m| m.entrypoint.clone())
            .unwrap_or_default();
    }

    let final_title = title
        .or_else(|| infer_title(&messages))
        .unwrap_or_else(|| format!("会话 {}", &session_id[..8.min(session_id.len())]));

    let mut role_counts = HashMap::new();
    for msg in &messages {
        *role_counts.entry(msg.role.clone()).or_insert(0) += 1;
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

    Ok(SessionSummary {
        id: session_id,
        title: final_title,
        path: path.display().to_string(),
        size: file_size,
        started_at,
        updated_at,
        cwd,
        model,
        source,
        forked_from: String::new(),
        messages,
        tools,
        role_counts,
        keywords: extract_keywords(&corpus, STOP_WORDS_CLAUDE),
        total_tokens,
        documents: Vec::new(),
    })
}

fn extract_claude_content(value: Option<&Value>) -> String {
    let value = match value {
        Some(v) => v,
        None => return String::new(),
    };
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    if let Some(arr) = value.as_array() {
        return arr
            .iter()
            .filter_map(|item| match item.get("type").and_then(|v| v.as_str()) {
                Some("text") => item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                Some("tool_result") => item
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

fn is_internal_claude(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<system-reminder")
        || t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<local-command-stdout>")
}

fn format_ts(ts: Option<f64>) -> String {
    match ts {
        Some(ms) => {
            let secs = (ms / 1000.0) as i64;
            let days = secs / 86400;
            let rem = secs % 86400;
            let h = rem / 3600;
            let m = (rem % 3600) / 60;
            let s = rem % 60;
            let base_days: i64 = 719468;
            let total_days = base_days + days;
            let (year, month, day) = days_to_ymd(total_days);
            format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
        }
        None => String::new(),
    }
}
