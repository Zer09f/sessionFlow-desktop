export interface HistoryResponse {
  root: string;
  sessions: CodexSession[];
  skipped: SkippedFile[];
}

export interface SkippedFile {
  path: string;
  reason: string;
}

export interface CodexSession {
  id: string;
  title: string;
  path: string;
  size: number;
  started_at: string;
  updated_at: string;
  cwd: string;
  model: string;
  source: string;
  forked_from: string;
  messages: CodexMessage[];
  tools: ToolCall[];
  role_counts: Record<string, number>;
  keywords: Keyword[];
  total_tokens: number | null;
  documents: DocumentInfo[];
}

export interface DocumentInfo {
  path: string;
  doc_type: string;
  action: string;
  edits: number;
}

export interface CodexMessage {
  role: "user" | "assistant" | "tool" | "system";
  text: string;
  timestamp: string;
}

export interface ToolCall {
  name: string;
  timestamp: string;
  call_id: string;
}

export interface Keyword {
  word: string;
  count: number;
}

export type SortMode = "updated-desc" | "updated-asc" | "messages-desc" | "tools-desc";
export type RangeMode = "all" | "7" | "30" | "90";

export interface SyncStatus {
  config_provider: string;
  config_model: string;
  db_mismatched: number;
  file_mismatched: number;
  index_missing: number;
  total_threads: number;
  needs_sync: boolean;
}

export interface SyncResult {
  db_updated: number;
  files_updated: number;
  index_rebuilt: boolean;
  backup_path: string;
}

export interface BackupInfo {
  backup_path: string;
  db_backed_up: boolean;
  index_backed_up: boolean;
  session_meta_count: number;
  timestamp: string;
}
