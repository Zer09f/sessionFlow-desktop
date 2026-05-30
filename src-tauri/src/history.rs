use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const MAX_JSONL_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub title: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub root: String,
    pub sessions: Vec<SessionSummary>,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Debug, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub path: String,
    pub size: u64,
    pub started_at: String,
    pub updated_at: String,
    pub cwd: String,
    pub model: String,
    pub source: String,
    pub forked_from: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolCall>,
    pub role_counts: HashMap<String, usize>,
    pub keywords: Vec<Keyword>,
    pub total_tokens: Option<u64>,
    pub documents: Vec<DocumentInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentInfo {
    pub path: String,
    pub doc_type: String,
    pub action: String,
    pub edits: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub timestamp: String,
    pub call_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Keyword {
    pub word: String,
    pub count: usize,
}

// ── Shared utilities (used by claude_history.rs, opencode_history.rs) ──

pub fn days_to_ymd(mut total_days: i64) -> (i64, i64, i64) {
    let era = (if total_days >= 0 {
        total_days
    } else {
        total_days - 146096
    }) / 146097;
    total_days -= era * 146097;
    let yoe = (total_days - total_days / 1460 + total_days / 36524 - total_days / 146096) / 365;
    let mut doy = total_days - (365 * yoe + yoe / 4 - yoe / 100);
    let mut year = yoe + era * 400;
    if doy < 0 {
        year -= 1;
        doy += 365
            + if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                1
            } else {
                0
            };
    }
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month as i64, day as i64)
}

pub fn epoch_to_iso(epoch: i64) -> String {
    if epoch <= 0 {
        return String::new();
    }
    let secs = if epoch > 1_000_000_000_000 {
        epoch / 1000
    } else {
        epoch
    };
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let total_days = 719468 + days;
    let (year, month, day) = days_to_ymd(total_days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn collect_jsonl_files(dir: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, output)?;
        } else if path
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("jsonl"))
        {
            output.push(path);
        }
    }
    Ok(())
}

pub fn str_value(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(ToString::to_string)
}

pub fn compact_text(value: &str, limit: usize) -> String {
    let mut output = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if output.chars().count() > limit {
        output = output
            .chars()
            .take(limit.saturating_sub(1))
            .collect::<String>();
        output.push('…');
    }
    output
}

pub fn clean_title(value: &str, limit: usize) -> Option<String> {
    let cleaned = compact_text(&value.replace('\r', " ").replace('\n', " "), limit);
    (!cleaned.trim().is_empty()).then_some(cleaned)
}

pub fn infer_title(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.role == "user")
        .or_else(|| messages.first())
        .and_then(|m| clean_title(&m.text, 120))
}

pub fn is_cjk(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
        || ('\u{3400}'..='\u{4DBF}').contains(&ch)
        || ('\u{20000}'..='\u{2A6DF}').contains(&ch)
}

pub fn extract_keywords(text: &str, stops: &[&str]) -> Vec<Keyword> {
    let stops: HashSet<&str> = stops.iter().copied().collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut ascii = String::new();
    let mut han = String::new();

    let flush_ascii =
        |word: &mut String, counts: &mut HashMap<String, usize>, stops: &HashSet<&str>| {
            if word.len() >= 3 && !stops.contains(word.as_str()) {
                *counts.entry(word.clone()).or_insert(0) += 1;
            }
            word.clear();
        };

    let flush_han =
        |word: &mut String, counts: &mut HashMap<String, usize>, stops: &HashSet<&str>| {
            let chars = word.chars().collect::<Vec<_>>();
            if chars.len() >= 2 {
                if chars.len() <= 6 {
                    if !stops.contains(word.as_str()) {
                        *counts.entry(word.clone()).or_insert(0) += 1;
                    }
                } else {
                    for window in chars.windows(2) {
                        let item: String = window.iter().collect();
                        if !stops.contains(item.as_str()) {
                            *counts.entry(item).or_insert(0) += 1;
                        }
                    }
                }
            }
            word.clear();
        };

    for ch in text.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            flush_han(&mut han, &mut counts, &stops);
            ascii.push(ch);
        } else if is_cjk(ch) {
            flush_ascii(&mut ascii, &mut counts, &stops);
            han.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut counts, &stops);
            flush_han(&mut han, &mut counts, &stops);
        }
    }
    flush_ascii(&mut ascii, &mut counts, &stops);
    flush_han(&mut han, &mut counts, &stops);

    let mut entries = counts
        .into_iter()
        .map(|(word, count)| Keyword { word, count })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    entries.truncate(18);
    entries
}

// ── Codex-specific logic ──

static STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "you", "your", "codex", "session",
    "message", "function", "output", "input", "text", "一个", "这个", "那个", "我们", "你们",
    "他们", "进行", "可以", "需要", "以及", "如果", "因为", "所以", "然后", "历史", "会话", "记录",
    "页面", "项目", "文件",
];

pub fn load(custom_root: Option<String>) -> Result<HistoryResponse, String> {
    let root = custom_root
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_codex_root);
    let root = root
        .canonicalize()
        .map_err(|err| format!("无法访问 Codex 数据目录 {}: {err}", root.display()))?;

    let index_path = root.join("session_index.jsonl");
    let sessions_dir = root.join("sessions");
    let index = if index_path.exists() {
        parse_index(&index_path)?
    } else {
        HashMap::new()
    };

    let mut rollout_files = Vec::new();
    if sessions_dir.exists() {
        collect_jsonl_files(&sessions_dir, &mut rollout_files)
            .map_err(|err| format!("扫描 sessions 目录失败: {err}"))?;
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

    let index_ref = &index;
    let root_ref = &root;
    let mut sessions: Vec<SessionSummary> = files_with_size
        .par_iter()
        .filter_map(|(file_path, size)| parse_rollout(file_path, root_ref, *size, index_ref).ok())
        .collect();

    let seen: HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();

    for (id, entry) in index {
        if seen.contains(&id) {
            continue;
        }
        sessions.push(SessionSummary {
            id: id.clone(),
            title: clean_title(&entry.title, 120)
                .unwrap_or_else(|| format!("会话 {}", short_id(&id))),
            path: "session_index.jsonl".to_string(),
            size: 0,
            started_at: entry.updated_at.clone(),
            updated_at: entry.updated_at,
            cwd: String::new(),
            model: "Codex".to_string(),
            source: "index".to_string(),
            forked_from: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            role_counts: HashMap::new(),
            keywords: extract_keywords(&entry.title, STOP_WORDS),
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

pub fn default_codex_root() -> PathBuf {
    if let Ok(value) = env::var("CODEX_HOME") {
        return PathBuf::from(value);
    }
    if let Ok(value) = env::var("USERPROFILE") {
        return PathBuf::from(value).join(".codex");
    }
    if let Ok(value) = env::var("HOME") {
        return PathBuf::from(value).join(".codex");
    }
    PathBuf::from(".codex")
}

pub fn parse_index(path: &Path) -> Result<HashMap<String, IndexEntry>, String> {
    let file = File::open(path).map_err(|err| format!("无法打开索引 {}: {err}", path.display()))?;
    let reader = BufReader::new(file);
    let mut output = HashMap::new();

    for line in reader.lines() {
        let line = line.map_err(|err| format!("读取索引失败: {err}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(id) = str_value(&value, &["id"]) {
            output.insert(
                id,
                IndexEntry {
                    title: str_value(&value, &["thread_name"]).unwrap_or_default(),
                    updated_at: str_value(&value, &["updated_at"]).unwrap_or_default(),
                },
            );
        }
    }

    Ok(output)
}

fn parse_rollout(
    path: &Path,
    root: &Path,
    size: u64,
    index: &HashMap<String, IndexEntry>,
) -> Result<SessionSummary, String> {
    let file = File::open(path).map_err(|err| format!("无法打开会话 {}: {err}", path.display()))?;
    let reader = BufReader::with_capacity(256 * 1024, file);
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();

    let mut id = extract_session_id(filename).unwrap_or_else(|| fallback_id(filename));
    let mut started_at = date_from_filename(filename).unwrap_or_default();
    let mut cwd = String::new();
    let mut model = String::from("Codex");
    let mut source = String::new();
    let mut forked_from = String::new();
    let mut last_timestamp = String::new();
    let mut messages = Vec::new();
    let mut fallback_messages = Vec::new();
    let mut tools = Vec::new();
    let mut tool_outputs = 0usize;
    let mut total_tokens = None;
    let mut document_map: HashMap<String, DocumentInfo> = HashMap::new();

    for line in reader.lines() {
        let line = line.map_err(|err| format!("读取会话失败 {}: {err}", path.display()))?;
        if line.is_empty() {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
            last_timestamp = ts.to_string();
        }

        match value.get("type").and_then(|v| v.as_str()) {
            Some("session_meta") => {
                let payload = &value["payload"];
                if let Some(next_id) = str_value(payload, &["id"]) {
                    id = next_id;
                }
                if let Some(meta_started) = str_value(payload, &["timestamp"]) {
                    started_at = meta_started;
                }
                if let Some(c) = str_value(payload, &["cwd"]) {
                    cwd = c;
                }
                model = str_value(payload, &["model"])
                    .or_else(|| str_value(payload, &["model_provider"]))
                    .or_else(|| str_value(payload, &["originator"]))
                    .unwrap_or(model);
                source = str_value(payload, &["source"])
                    .or_else(|| str_value(payload, &["thread_source"]))
                    .unwrap_or(source);
                if let Some(f) = str_value(payload, &["forked_from_id"]) {
                    forked_from = f;
                }
            }
            Some("turn_context") => {
                if let Some(c) = str_value(&value, &["payload", "cwd"]) {
                    cwd = c;
                }
            }
            Some("response_item") => {
                let payload = &value["payload"];
                match payload.get("type").and_then(|v| v.as_str()) {
                    Some("message") => {
                        let text = extract_content(&payload["content"]);
                        if !text.trim().is_empty() && !is_internal_context(&text) {
                            let role = match payload.get("role").and_then(|v| v.as_str()) {
                                Some("assistant") => "assistant",
                                Some("tool") => "tool",
                                Some("system") => "system",
                                _ => "user",
                            };
                            messages.push(Message {
                                role: role.to_string(),
                                text: compact_text(&text, 12_000),
                                timestamp: last_timestamp.clone(),
                            });
                        }
                    }
                    Some("function_call") => {
                        let tool_name = payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        tools.push(ToolCall {
                            name: tool_name.clone(),
                            timestamp: last_timestamp.clone(),
                            call_id: payload
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        });

                        if tool_name == "write_to_file"
                            || tool_name == "replace_file_content"
                            || tool_name == "multi_replace_file_content"
                        {
                            if let Some(args_str) = str_value(payload, &["arguments"]) {
                                if let Ok(args_val) = serde_json::from_str::<Value>(&args_str) {
                                    if let Some(target_file) = str_value(&args_val, &["TargetFile"])
                                    {
                                        let action = if tool_name == "write_to_file" {
                                            "create"
                                        } else {
                                            "edit"
                                        };
                                        let doc_type = str_value(
                                            &args_val,
                                            &["ArtifactMetadata", "ArtifactType"],
                                        )
                                        .map(|t| format!("Artifact: {}", t))
                                        .unwrap_or_else(|| "Source File".to_string());

                                        let entry = document_map
                                            .entry(target_file.clone())
                                            .or_insert(DocumentInfo {
                                                path: target_file,
                                                doc_type,
                                                action: action.to_string(),
                                                edits: 0,
                                            });
                                        if action == "edit" {
                                            entry.action = "edit".to_string();
                                        }
                                        entry.edits += 1;
                                    }
                                }
                            }
                        }
                    }
                    Some("function_call_output") => {
                        tool_outputs += 1;
                    }
                    _ => {}
                }
            }
            Some("event_msg") => {
                let payload = &value["payload"];
                match payload.get("type").and_then(|v| v.as_str()) {
                    Some("user_message") | Some("agent_message") => {
                        let text = str_value(payload, &["message"]).unwrap_or_default();
                        if !text.trim().is_empty() && !is_internal_context(&text) {
                            let role = if payload.get("type").and_then(|v| v.as_str())
                                == Some("user_message")
                            {
                                "user"
                            } else {
                                "assistant"
                            };
                            fallback_messages.push(Message {
                                role: role.to_string(),
                                text: compact_text(&text, 12_000),
                                timestamp: last_timestamp.clone(),
                            });
                        }
                    }
                    Some("token_count") => {
                        total_tokens = value
                            .pointer("/payload/info/total_token_usage/total_tokens")
                            .or_else(|| {
                                value.pointer("/payload/info/last_token_usage/total_tokens")
                            })
                            .and_then(|v| v.as_u64())
                            .or(total_tokens);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    if messages.is_empty() {
        messages = fallback_messages;
    }

    let index_entry = index.get(&id);
    let updated_at = index_entry
        .map(|entry| entry.updated_at.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| (!last_timestamp.is_empty()).then(|| last_timestamp))
        .or_else(|| (!started_at.is_empty()).then(|| started_at.clone()))
        .unwrap_or_default();
    if started_at.is_empty() {
        started_at = updated_at.clone();
    }

    let title = index_entry
        .and_then(|entry| clean_title(&entry.title, 120))
        .or_else(|| infer_title(&messages))
        .unwrap_or_else(|| format!("会话 {}", short_id(&id)));

    let mut role_counts = HashMap::new();
    for message in &messages {
        *role_counts.entry(message.role.clone()).or_insert(0) += 1;
    }
    if tool_outputs > 0 {
        *role_counts.entry("tool".to_string()).or_insert(0) += tool_outputs;
    }

    let corpus = format!(
        "{}\n{}",
        title,
        messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );

    Ok(SessionSummary {
        id,
        title,
        path: path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
        size,
        started_at,
        updated_at,
        cwd,
        model,
        source,
        forked_from,
        messages,
        tools,
        role_counts,
        keywords: extract_keywords(&corpus, STOP_WORDS),
        total_tokens,
        documents: document_map.into_values().collect(),
    })
}

fn extract_content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(parts) = value.as_array() {
        return parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("input_text"))
                    .or_else(|| part.get("output_text"))
                    .and_then(|v| v.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

fn is_internal_context(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<environment_context")
        || t.starts_with("<developer_context")
        || t.starts_with("<skills_instructions")
        || t.starts_with("<plugins_instructions")
}

fn extract_session_id(name: &str) -> Option<String> {
    let chars = name.chars().collect::<Vec<_>>();
    for start in 0..chars.len().saturating_sub(35) {
        let slice = chars[start..start + 36].iter().collect::<String>();
        if looks_like_uuid(&slice) {
            return Some(slice);
        }
    }
    None
}

fn looks_like_uuid(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().get(8) == Some(&b'-')
        && value.as_bytes().get(13) == Some(&b'-')
        && value.as_bytes().get(18) == Some(&b'-')
        && value.as_bytes().get(23) == Some(&b'-')
        && value
            .bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 8 | 13 | 18 | 23) || b.is_ascii_hexdigit())
}

fn date_from_filename(name: &str) -> Option<String> {
    let rest = name.strip_prefix("rollout-")?;
    let raw = rest.get(0..19)?;
    if raw.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    Some(format!(
        "{}T{}:{}:{}Z",
        &raw[0..10],
        &raw[11..13],
        &raw[14..16],
        &raw[17..19]
    ))
}

fn fallback_id(seed: &str) -> String {
    let mut hash = 2166136261u32;
    for byte in seed.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    format!("local-{hash:x}")
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
