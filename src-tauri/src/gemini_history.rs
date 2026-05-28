use rayon::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::history::{
    compact_text, extract_keywords, infer_title, HistoryResponse, Message, SessionSummary,
    ToolCall,
};

static STOP_WORDS_GEMINI: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "you", "your", "gemini",
    "session", "message", "function", "output", "input", "text", "type", "content", "tool", "file",
    "code", "true", "false", "null", "name", "value", "一个", "这个", "那个", "我们", "你们",
    "他们", "进行", "可以", "需要", "以及", "如果", "因为", "所以", "然后", "历史", "会话", "记录",
];

pub fn load(custom_root: Option<String>) -> Result<HistoryResponse, String> {
    let root = custom_root
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_gemini_root);
    let root = root
        .canonicalize()
        .map_err(|e| format!("无法访问 Gemini CLI 数据目录 {}: {e}", root.display()))?;

    let tmp_dir = root.join("tmp");
    if !tmp_dir.exists() {
        return Ok(HistoryResponse {
            root: root.display().to_string(),
            sessions: Vec::new(),
            skipped: Vec::new(),
        });
    }

    // 收集所有 chats/session-*.json 文件
    let mut session_files: Vec<(PathBuf, u64)> = Vec::new();
    collect_session_files(&tmp_dir, &mut session_files);

    let mut sessions: Vec<SessionSummary> = session_files
        .par_iter()
        .filter_map(|(path, size)| parse_session_file(path, *size).ok())
        .collect();

    // 如果 chats/ 为空，回退到 logs.json
    if sessions.is_empty() {
        load_from_logs(&tmp_dir, &mut sessions)?;
    }

    // 去重 + 排序
    let mut seen = HashSet::new();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.retain(|s| seen.insert(s.id.clone()));

    Ok(HistoryResponse {
        root: root.display().to_string(),
        sessions,
        skipped: Vec::new(),
    })
}

fn default_gemini_root() -> PathBuf {
    if let Ok(v) = env::var("GEMINI_CLI_HOME") {
        let p = PathBuf::from(v).join(".gemini");
        if p.exists() {
            return p;
        }
    }
    if let Ok(v) = env::var("USERPROFILE") {
        return PathBuf::from(v).join(".gemini");
    }
    if let Ok(v) = env::var("HOME") {
        return PathBuf::from(v).join(".gemini");
    }
    PathBuf::from(".gemini")
}

fn collect_session_files(tmp_dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let entries = match fs::read_dir(tmp_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let chats_dir = path.join("chats");
        if !chats_dir.exists() {
            continue;
        }
        if let Ok(chat_entries) = fs::read_dir(&chats_dir) {
            for chat_entry in chat_entries.flatten() {
                let fp = chat_entry.path();
                if fp.extension().and_then(|v| v.to_str()) != Some("json") {
                    continue;
                }
                let name = fp.file_name().and_then(|v| v.to_str()).unwrap_or("");
                if !name.starts_with("session-") && !name.starts_with("checkpoint-") {
                    continue;
                }
                if let Ok(meta) = fs::metadata(&fp) {
                    out.push((fp, meta.len()));
                }
            }
        }
    }
}

fn parse_session_file(path: &Path, file_size: u64) -> Result<SessionSummary, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("读取 {}: {e}", path.display()))?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 {}: {e}", path.display()))?;

    let session_id = value
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Err("sessionId 为空".to_string());
    }

    let started_at = str_val(&value, "startTime");
    let updated_at = str_val(&value, "lastUpdated");

    // 从项目目录读取 .project_root
    let parent_dir = path.parent().and_then(|p| p.parent());
    let cwd = parent_dir
        .and_then(|d| fs::read_to_string(d.join(".project_root")).ok())
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut messages: Vec<Message> = Vec::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    let mut model = String::new();
    let mut tool_outputs = 0usize;
    let mut total_tokens: Option<u64> = None;

    if let Some(msgs) = value.get("messages").and_then(|v| v.as_array()) {
        for msg in msgs {
            let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let ts = str_val(msg, "timestamp");

            match msg_type {
                "user" => {
                    let text = str_val(msg, "content");
                    if !text.trim().is_empty() && !text.starts_with('/') {
                        messages.push(Message {
                            role: "user".to_string(),
                            text: compact_text(&text, 12_000),
                            timestamp: ts,
                        });
                    }
                }
                "gemini" => {
                    if model.is_empty() {
                        model = str_val(msg, "model");
                    }
                    if let Some(tokens) = msg.get("tokens") {
                        let total = tokens.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                        if total > 0 {
                            total_tokens = Some(total_tokens.unwrap_or(0) + total);
                        }
                    }
                    let text = str_val(msg, "content");
                    if !text.trim().is_empty() {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            text: compact_text(&text, 12_000),
                            timestamp: ts.clone(),
                        });
                    }
                    if let Some(calls) = msg.get("toolCalls").and_then(|v| v.as_array()) {
                        for tc in calls {
                            let name = str_val(tc, "name");
                            let call_id = str_val(tc, "id");
                            let tc_ts = str_val(tc, "timestamp");
                            if !name.is_empty() {
                                tools.push(ToolCall {
                                    name,
                                    timestamp: if tc_ts.is_empty() { ts.clone() } else { tc_ts },
                                    call_id,
                                });
                                tool_outputs += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let title = infer_title(&messages)
        .unwrap_or_else(|| format!("会话 {}", &session_id[..8.min(session_id.len())]));

    let mut role_counts = HashMap::new();
    for m in &messages {
        *role_counts.entry(m.role.clone()).or_insert(0usize) += 1;
    }
    if tool_outputs > 0 {
        *role_counts.entry("tool".to_string()).or_insert(0) += tool_outputs;
    }

    let corpus = format!(
        "{}\n{}",
        &title,
        messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );

    Ok(SessionSummary {
        id: session_id,
        title,
        path: path.display().to_string(),
        size: file_size,
        started_at,
        updated_at,
        cwd,
        model: if model.is_empty() {
            "Gemini".to_string()
        } else {
            model
        },
        source: "gemini".to_string(),
        forked_from: String::new(),
        messages,
        tools,
        role_counts,
        keywords: extract_keywords(&corpus, STOP_WORDS_GEMINI),
        total_tokens,
        documents: Vec::new(),
    })
}

/// 回退：从 logs.json 加载（仅用户消息）
fn load_from_logs(tmp_dir: &Path, sessions: &mut Vec<SessionSummary>) -> Result<(), String> {
    let entries = match fs::read_dir(tmp_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    let mut by_session: HashMap<String, Vec<(String, String)>> = HashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let logs_path = path.join("logs.json");
        if !logs_path.exists() {
            continue;
        }

        let cwd = fs::read_to_string(path.join(".project_root"))
            .unwrap_or_default()
            .trim()
            .to_string();

        let content = match fs::read_to_string(&logs_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let items: Vec<Value> = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        for item in &items {
            let sid = str_val(item, "sessionId");
            if sid.is_empty() {
                continue;
            }
            let msg_type = str_val(item, "type");
            if msg_type != "user" {
                continue;
            }
            let text = str_val(item, "message");
            let ts = str_val(item, "timestamp");
            if !text.trim().is_empty() && !text.starts_with('/') {
                by_session
                    .entry(sid)
                    .or_default()
                    .push((text, ts));
            }
        }

        // 附加 cwd 到 session（取最后一条消息的 sid）
        if let Some(last) = items.last() {
            let sid = str_val(last, "sessionId");
            if !cwd.is_empty() && !sid.is_empty() {
                // 存在但不在这里修改，等构建 session 时使用
                let _ = (sid, cwd);
            }
        }
    }

    for (sid, msgs) in by_session {
        let mut messages: Vec<Message> = Vec::new();
        let mut last_ts = String::new();
        let mut first_ts = String::new();

        for (text, ts) in &msgs {
            if first_ts.is_empty() {
                first_ts = ts.clone();
            }
            last_ts = ts.clone();
            messages.push(Message {
                role: "user".to_string(),
                text: compact_text(text, 12_000),
                timestamp: ts.clone(),
            });
        }

        if first_ts.is_empty() {
            first_ts = last_ts.clone();
        }

        let title = infer_title(&messages)
            .unwrap_or_else(|| format!("会话 {}", &sid[..8.min(sid.len())]));

        let mut role_counts = HashMap::new();
        for m in &messages {
            *role_counts.entry(m.role.clone()).or_insert(0usize) += 1;
        }

        let corpus = format!(
            "{}\n{}",
            &title,
            messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );

        sessions.push(SessionSummary {
            id: sid,
            title,
            path: String::new(),
            size: 0,
            started_at: first_ts,
            updated_at: last_ts,
            cwd: String::new(),
            model: "Gemini".to_string(),
            source: "gemini".to_string(),
            forked_from: String::new(),
            messages,
            tools: Vec::new(),
            role_counts,
            keywords: extract_keywords(&corpus, STOP_WORDS_GEMINI),
            total_tokens: None,
            documents: Vec::new(),
        });
    }

    Ok(())
}

fn str_val(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}
