<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { CodexMessage, CodexSession, HistoryResponse, RangeMode, SortMode, SyncStatus, SyncResult, BackupInfo } from "./types";

type ExportFormat = "json" | "markdown";
const isTauri = "__TAURI_INTERNALS__" in window;
const exportFormat = ref<ExportFormat>("json");
const showExportMenu = ref(false);

type SourceTab = "codex" | "claude" | "opencode" | "gemini";

interface TabMeta {
  label: string;
  sourceLabel: string;
  icon: string;
  command: string;
  defaultDir: string;
  dirDesc: string;
}

const activeTab = ref<SourceTab>("codex");
const showContent = ref(true);
const expandAllMessages = ref(false);
const messageLimit = ref(50);
const messageRoleFilter = ref<"all" | "assistant" | "user">("all");
const theme = ref<"light" | "dark">("light");

// ── Codex 同步恢复状态 ──
const syncStatus = ref<SyncStatus | null>(null);
const syncLoading = ref(false);
const syncResult = ref<SyncResult | null>(null);
const syncError = ref("");

const stateMap: Record<SourceTab, ReturnType<typeof useSourceState>> = {
  codex: useSourceState(),
  claude: useSourceState(),
  opencode: useSourceState(),
  gemini: useSourceState(),
};

function useSourceState() {
  return {
    sessions: ref<CodexSession[]>([]),
    selectedId: ref(""),
    query: ref(""),
    range: ref<RangeMode>("all"),
    sortMode: ref<SortMode>("updated-desc"),
    docFilter: ref<"all" | "with-docs" | "no-docs">("all"),
    rootPath: ref(""),
    loading: ref(false),
    loaded: ref(false),
    status: ref("尚未读取历史记录。"),
    statusError: ref(false),
  };
}

function src() {
  return stateMap[activeTab.value];
}

const loading = computed(() => src().loading.value);
const loaded = computed(() => src().loaded.value);
const status = computed(() => src().status.value);
const statusError = computed(() => src().statusError.value);

const roleColors: Record<string, string> = {
  user: "var(--role-user)",
  assistant: "var(--role-assistant)",
  tool: "var(--role-tool)",
  system: "var(--role-system)",
};

const tabMeta: Record<SourceTab, TabMeta> = {
  codex: {
    label: "Codex",
    sourceLabel: "Codex",
    icon: "⬡",
    command: "load_codex_history",
    defaultDir: "~/.codex",
    dirDesc: ".codex/session_index.jsonl 与 sessions/**/*.jsonl",
  },
  claude: {
    label: "Claude Code",
    sourceLabel: "Claude",
    icon: "✦",
    command: "load_claude_history",
    defaultDir: "~/.claude",
    dirDesc: ".claude/sessions/*.json 与 projects/**/*.jsonl",
  },
  opencode: {
    label: "OpenCode",
    sourceLabel: "OpenCode",
    icon: "◈",
    command: "load_opencode_history",
    defaultDir: "~/.local/share/opencode",
    dirDesc: "storage/session/*.json 与 storage/message/*.json 或 SQLite",
  },
  gemini: {
    label: "Gemini CLI",
    sourceLabel: "Gemini",
    icon: "◆",
    command: "load_gemini_history",
    defaultDir: "~/.gemini",
    dirDesc: "tmp/<project>/chats/session-*.json 与 logs.json",
  },
};

const filteredSessions = computed(() => {
  const s = src().sessions.value;
  const rangeDays = src().range.value === "all" ? null : Number(src().range.value);
  const minTime = rangeDays ? Date.now() - rangeDays * 24 * 60 * 60 * 1000 : 0;
  const needle = src().query.value.trim().toLowerCase();
  const docF = src().docFilter.value;

  return [...s]
    .filter((item) => dateOf(item.updated_at).getTime() >= minTime)
    .filter((item) => {
      if (docF === "with-docs") return item.documents && item.documents.length > 0;
      if (docF === "no-docs") return !item.documents || item.documents.length === 0;
      return true;
    })
    .filter((item) => {
      if (!needle) return true;
      // 只搜索标题、目录、路径和关键词，不拼接全部消息文本
      const haystack = [item.title, item.cwd, item.path, item.keywords.map((k) => k.word).join(" ")]
        .join(" ")
        .toLowerCase();
      if (haystack.includes(needle)) return true;
      // 关键词未命中时，再逐条搜索消息（短路退出）
      return item.messages.some((m) => m.text.toLowerCase().includes(needle));
    })
    .sort((a, b) => sortSessions(a, b, src().sortMode.value));
});

const selectedSession = computed(() => filteredSessions.value.find((item) => item.id === src().selectedId.value) ?? null);

const relativeTimeMap = computed(() => {
  const map: Record<string, string> = {};
  for (const item of filteredSessions.value) {
    map[item.id] = formatRelativeTime(item.updated_at);
  }
  return map;
});

const filteredTimelineMessages = computed(() => {
  if (!selectedSession.value) return [];
  return selectedSession.value.messages.filter((msg) => {
    if (messageRoleFilter.value === "all") return true;
    return msg.role === messageRoleFilter.value;
  });
});

const summary = computed(() => {
  const items = filteredSessions.value;
  const roleCounts = aggregateRoles(items);
  const toolCounts = aggregateTools(items);
  const days = aggregateDays(items);
  const dates = items.map((item) => dateOf(item.updated_at)).filter((date) => date.getTime() > 0).sort((a, b) => a.getTime() - b.getTime());
  const sourceLabel = tabMeta[activeTab.value].sourceLabel;

  return {
    sessions: items.length,
    messages: items.reduce((sum, item) => sum + item.messages.length, 0),
    tools: items.reduce((sum, item) => sum + item.tools.length, 0),
    documents: items.reduce((sum, item) => sum + (item.documents ? item.documents.length : 0), 0),
    totalTokens: items.reduce((sum, item) => sum + (item.total_tokens ?? 0), 0),
    sessionsWithTokens: items.filter((item) => item.total_tokens != null).length,
    activeDays: Object.keys(days).length,
    users: roleCounts.user ?? 0,
    assistants: roleCounts.assistant ?? 0,
    topTool: topEntry(toolCounts),
    peakDay: topEntry(days),
    rangeLabel: dates.length ? `${formatDate(dates[0])} 至 ${formatDate(dates.at(-1)!)}` : "等待读取",
    sourceLabel,
  };
});

const dayEntries = computed(() => {
  const counts = aggregateDays(filteredSessions.value);
  const keys = Object.keys(counts).sort();
  if (!keys.length) return [];

  const rangeDays = src().range.value === "all" ? null : Number(src().range.value);
  const today = formatDate(new Date());

  const entries: Array<{ day: string; count: number }> = [];
  const end = new Date(`${today}T00:00:00`);
  const start = rangeDays
    ? new Date(end.getTime() - (rangeDays - 1) * 24 * 60 * 60 * 1000)
    : new Date(`${keys[0]}T00:00:00`);

  for (const cursor = new Date(start); cursor <= end; cursor.setDate(cursor.getDate() + 1)) {
    const key = formatDate(cursor);
    entries.push({ day: key, count: counts[key] ?? 0 });
  }

  return rangeDays ? entries : entries.slice(-90);
});

const maxDayCount = computed(() => Math.max(...dayEntries.value.map((entry) => entry.count), 1));

const roleEntries = computed(() => {
  const counts = aggregateRoles(filteredSessions.value);
  const sourceLabel = tabMeta[activeTab.value].sourceLabel;
  return [
    { key: "user", label: "用户", count: counts.user ?? 0 },
    { key: "assistant", label: sourceLabel, count: counts.assistant ?? 0 },
    { key: "tool", label: "工具输出", count: counts.tool ?? 0 },
    { key: "system", label: "系统", count: counts.system ?? 0 },
  ].filter((item) => item.count > 0);
});

const roleTotal = computed(() => roleEntries.value.reduce((s, e) => s + e.count, 0));

const roleDonut = computed(() => {
  const total = roleTotal.value;
  if (!total) return "conic-gradient(var(--line) 0 100%)";

  let current = 0;
  const parts = roleEntries.value.map((item) => {
    const start = current;
    current += (item.count / total) * 100;
    return `${roleColors[item.key]} ${start}% ${current}%`;
  });
  return `conic-gradient(${parts.join(", ")})`;
});

const toolEntries = computed(() => {
  return Object.entries(aggregateTools(filteredSessions.value))
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, 10);
});

const maxToolCount = computed(() => Math.max(...toolEntries.value.map((entry) => entry[1]), 1));

const hourEntries = computed(() => {
  const counts = Array.from({ length: 24 }, (_, hour) => ({ hour, count: 0 }));
  filteredSessions.value.forEach((item) => {
    const date = dateOf(item.updated_at);
    if (date.getTime() > 0) counts[date.getHours()].count += 1;
  });
  const max = Math.max(...counts.map((item) => item.count), 1);
  return counts.map((item) => ({
    ...item,
    intensity: item.count ? 18 + Math.round((item.count / max) * 72) : 0,
  }));
});

const detailMeta = computed(() => {
  if (!selectedSession.value) return [];
  const item = selectedSession.value;
  return [
    ["消息", String(item.messages.length)],
    ["工具调用", String(item.tools.length)],
    ["持续时间", formatDuration(dateOf(item.updated_at).getTime() - dateOf(item.started_at).getTime())],
    ["工作目录", item.cwd || "未记录"],
    ["文件", item.path],
    ["大小", formatBytes(item.size)],
    ["来源", item.source || "rollout"],
    ["Token", item.total_tokens ? formatNumber(item.total_tokens) : "未记录"],
  ];
});

watch(filteredSessions, (items) => {
  const state = src();
  if (!items.some((item) => item.id === state.selectedId.value)) {
    state.selectedId.value = items[0]?.id ?? "";
  }
});

watch(
  () => src().selectedId.value,
  () => {
    expandAllMessages.value = false;
    messageLimit.value = 50;
    messageRoleFilter.value = "all";
  },
);

async function restoreSession() {
  if (!selectedSession.value) return;
  try {
    await invoke("restore_session", {
      cwd: selectedSession.value.cwd || "",
      id: selectedSession.value.id || "",
      source: activeTab.value,
      path: selectedSession.value.path || "",
    });
  } catch (err) {
    alert(`CLI 恢复: ${err}`);
  }
}

async function restoreViaClient() {
  if (!selectedSession.value) return;
  try {
    const client = await invoke<string>("restore_via_client", {
      cwd: selectedSession.value.cwd || "",
      source: activeTab.value,
    });
    alert(`已通过 ${client} 打开项目目录`);
  } catch (err) {
    alert(`客户端恢复: ${err}`);
  }
}

// ── Codex 同步恢复函数 ──
async function checkSyncStatus() {
  syncLoading.value = true;
  syncError.value = "";
  syncResult.value = null;
  try {
    syncStatus.value = await invoke<SyncStatus>("codex_sync_status", {
      root: stateMap.codex.rootPath.value.trim() || null,
    });
  } catch (err) {
    syncError.value = String(err);
  } finally {
    syncLoading.value = false;
  }
}

async function createSyncBackup() {
  syncLoading.value = true;
  syncError.value = "";
  try {
    const info = await invoke<BackupInfo>("codex_sync_backup", {
      root: stateMap.codex.rootPath.value.trim() || null,
    });
    alert(`备份已创建: ${info.backup_path}`);
  } catch (err) {
    syncError.value = String(err);
  } finally {
    syncLoading.value = false;
  }
}

async function executeSync() {
  if (!confirm("即将修改 Codex 数据库和会话文件。建议先创建备份。是否继续？")) return;
  syncLoading.value = true;
  syncError.value = "";
  try {
    syncResult.value = await invoke<SyncResult>("codex_sync_execute", {
      root: stateMap.codex.rootPath.value.trim() || null,
    });
    if (activeTab.value === "codex" && stateMap.codex.loaded.value) {
      await loadHistory();
    }
    await checkSyncStatus();
  } catch (err) {
    syncError.value = String(err);
  } finally {
    syncLoading.value = false;
  }
}

async function openDir(dir: string) {
  try {
    await invoke("open_path", { path: dir });
  } catch (err) {
    alert(`打开目录失败: ${err}`);
  }
}

async function openDoc(docPath: string) {
  if (!selectedSession.value?.cwd) return;
  const fullPath = docPath.startsWith("/") || docPath.match(/^[A-Za-z]:/)
    ? docPath
    : `${selectedSession.value.cwd}/${docPath}`;
  try {
    await invoke("open_path", { path: fullPath });
  } catch (err) {
    alert(`打开文件失败: ${err}`);
  }
}

function sortSessions(a: CodexSession, b: CodexSession, mode: SortMode) {
  if (mode === "updated-asc") return dateOf(a.updated_at).getTime() - dateOf(b.updated_at).getTime();
  if (mode === "messages-desc") return b.messages.length - a.messages.length || dateOf(b.updated_at).getTime() - dateOf(a.updated_at).getTime();
  if (mode === "tools-desc") return b.tools.length - a.tools.length || dateOf(b.updated_at).getTime() - dateOf(a.updated_at).getTime();
  return dateOf(b.updated_at).getTime() - dateOf(a.updated_at).getTime();
}

async function loadHistory() {
  const state = src();
  const meta = tabMeta[activeTab.value];
  state.loading.value = true;
  state.statusError.value = false;
  state.status.value = `正在通过 Rust 后端读取 ${meta.label} 历史记录...`;

  try {
    const response = await invoke<HistoryResponse>(meta.command, {
      root: state.rootPath.value.trim() || null,
    });
    state.sessions.value = response.sessions;
    state.selectedId.value = response.sessions[0]?.id ?? "";
    state.loaded.value = true;
    const skipped = response.skipped.length ? `，跳过 ${response.skipped.length} 个过大文件` : "";
    state.status.value = `已从 ${response.root} 读取 ${response.sessions.length} 个会话${skipped}。`;
  } catch (error) {
    state.statusError.value = true;
    state.status.value = `读取失败：${String(error)}`;
  } finally {
    state.loading.value = false;
  }
}

function loadSample() {
  const state = src();
  const now = Date.now();
  state.sessions.value = [
    makeSample("sample-1", "完善 VS Code 插件的 diff 响应", now - 1 * dayMs, 18, ["shell_command", "apply_patch"]),
    makeSample("sample-2", "整理 Rust 异步学习路线", now - 4 * dayMs, 11, ["shell_command"]),
    makeSample("sample-3", "设计 AI 代码变更可视化工具", now - 8 * dayMs, 25, ["shell_command", "spawn_agent", "wait_agent"]),
    makeSample("sample-4", "清理 Windows 磁盘空间", now - 11 * dayMs, 7, ["shell_command"]),
  ].sort((a, b) => dateOf(b.updated_at).getTime() - dateOf(a.updated_at).getTime());
  state.selectedId.value = state.sessions.value[0]?.id ?? "";
  state.statusError.value = false;
  state.loaded.value = true;
  state.status.value = "已载入示例数据。";
}

function buildExportJSON(): string {
  const meta = tabMeta[activeTab.value];
  const payload = {
    source: meta.label,
    exported_at: new Date().toISOString(),
    total_sessions: filteredSessions.value.length,
    sessions: filteredSessions.value.map((item) => ({
      id: item.id,
      title: item.title,
      started_at: item.started_at,
      updated_at: item.updated_at,
      cwd: item.cwd,
      model: item.model,
      source: item.source,
      messages: item.messages.map((m) => ({ role: m.role, text: m.text, timestamp: m.timestamp })),
      tools: item.tools.map((t) => ({ name: t.name, timestamp: t.timestamp, call_id: t.call_id })),
      keywords: item.keywords,
      documents: item.documents ?? [],
      total_tokens: item.total_tokens,
    })),
    total_tokens: summary.value.totalTokens,
  };
  return JSON.stringify(payload, null, 2);
}

function buildExportMarkdown(): string {
  const meta = tabMeta[activeTab.value];
  const now = formatDateTime(new Date().toISOString());
  const lines: string[] = [
    `# SessionFlow — ${meta.label} 历史导出`,
    "",
    `- 导出时间：${now}`,
    `- 数据源：${meta.label}`,
    `- 总会话数：${filteredSessions.value.length}`,
    "",
    "---",
    "",
  ];

  for (const item of filteredSessions.value) {
    lines.push(`## ${item.title || "未命名会话"}`);
    lines.push("");
    lines.push(`- **ID**: ${item.id}`);
    if (item.cwd) lines.push(`- **工作目录**: ${item.cwd}`);
    lines.push(`- **模型**: ${item.model || meta.sourceLabel}`);
    lines.push(`- **时间**: ${formatDateTime(item.started_at)} — ${formatDateTime(item.updated_at)}`);
    if (item.keywords.length) {
      lines.push(`- **关键词**: ${item.keywords.map((k) => `#${k.word}`).join(" ")}`);
    }
    lines.push("");

    if (item.messages.length) {
      lines.push("### 消息记录");
      lines.push("");
      for (const msg of item.messages) {
        const time = formatDateTime(msg.timestamp).split(" ").pop() || "";
        const role = msg.role === "user" ? "用户" : msg.role === "assistant" ? meta.sourceLabel : msg.role === "tool" ? "工具" : "系统";
        lines.push(`**[${time}] ${role}：**`);
        lines.push(msg.text);
        lines.push("");
      }
    }

    if (item.tools.length) {
      lines.push("### 工具调用");
      lines.push("");
      for (const tool of item.tools) {
        const time = formatDateTime(tool.timestamp).split(" ").pop() || "";
        lines.push(`- **[${time}]** ${tool.name}`);
      }
      lines.push("");
    }

    if (item.documents && item.documents.length) {
      lines.push("### 产出文档");
      lines.push("");
      for (const doc of item.documents) {
        const action = doc.action === "create" ? "新建" : "编辑";
        lines.push(`- ${doc.path} (${doc.doc_type}, ${action} ${doc.edits} 次)`);
      }
      lines.push("");
    }

    lines.push("---");
    lines.push("");
  }

  return lines.join("\n");
}

function downloadBlob(content: string, filename: string, mimeType: string) {
  const blob = new Blob([content], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  URL.revokeObjectURL(url);
}

async function exportData() {
  showExportMenu.value = false;
  const dateStr = formatDate(new Date());
  const fmt = exportFormat.value;

  const content = fmt === "json" ? buildExportJSON() : buildExportMarkdown();
  const ext = fmt === "json" ? "json" : "md";
  const mime = fmt === "json" ? "application/json" : "text/markdown";
  const defaultName = `${activeTab.value}-history-${dateStr}.${ext}`;

  if (isTauri) {
    try {
      const path = await invoke<string>("save_export", { filename: defaultName, content });
      const folder = path.replace(/[/\\][^/\\]+$/, "");
      alert(`已导出到: ${path}\n点击确定打开导出目录。`);
      await invoke("open_path", { path: folder });
    } catch (err) {
      alert(`导出失败: ${err}`);
    }
  } else {
    downloadBlob(content, defaultName, mime);
  }
}

const systemThemeMedia = window.matchMedia("(prefers-color-scheme: dark)");

function applyTheme(isDark: boolean) {
  theme.value = isDark ? "dark" : "light";
  document.documentElement.dataset.theme = isDark ? "dark" : "";
}

function toggleTheme() {
  applyTheme(theme.value !== "dark");
}

onMounted(() => {
  applyTheme(systemThemeMedia.matches);

  const themeListener = (e: MediaQueryListEvent) => {
    applyTheme(e.matches);
  };

  systemThemeMedia.addEventListener("change", themeListener);
  onUnmounted(() => {
    systemThemeMedia.removeEventListener("change", themeListener);
  });
});

function aggregateRoles(items: CodexSession[]) {
  return items.reduce<Record<string, number>>((acc, item) => {
    Object.entries(item.role_counts).forEach(([role, count]) => {
      acc[role] = (acc[role] ?? 0) + count;
    });
    return acc;
  }, {});
}

function aggregateTools(items: CodexSession[]) {
  return items.reduce<Record<string, number>>((acc, item) => {
    item.tools.forEach((tool) => {
      acc[tool.name] = (acc[tool.name] ?? 0) + 1;
    });
    return acc;
  }, {});
}

function aggregateDays(items: CodexSession[]) {
  return items.reduce<Record<string, number>>((acc, item) => {
    const key = formatDate(dateOf(item.updated_at));
    if (key !== "-") acc[key] = (acc[key] ?? 0) + 1;
    return acc;
  }, {});
}

function topEntry(values: Record<string, number>) {
  return Object.entries(values).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0] ?? null;
}

function dateOf(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? new Date(0) : date;
}

function formatNumber(value: number) {
  return new Intl.NumberFormat("zh-CN").format(value || 0);
}

function formatTokens(value: number) {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(value);
}

function formatDate(date: Date) {
  if (!(date instanceof Date) || Number.isNaN(date.getTime()) || date.getTime() <= 0) return "-";
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function formatDateTime(value: string) {
  const date = dateOf(value);
  if (date.getTime() <= 0) return "-";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

function formatDuration(ms: number) {
  if (!Number.isFinite(ms) || ms <= 0) return "瞬时";
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} 小时`;
  return `${Math.round(hours / 24)} 天`;
}

function formatBytes(bytes: number) {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

const dayMs = 24 * 60 * 60 * 1000;

function formatRelativeTime(value: string) {
  const date = dateOf(value);
  if (date.getTime() <= 0) return "";
  const diff = Date.now() - date.getTime();
  if (diff < 60 * 1000) return "刚刚";
  if (diff < 3600 * 1000) return `${Math.floor(diff / 60 / 1000)} 分钟前`;
  if (diff < dayMs) return `${Math.floor(diff / 3600 / 1000)} 小时前`;
  if (diff < 30 * dayMs) return `${Math.floor(diff / dayMs)} 天前`;
  return formatDate(date);
}

function roleLabel(role: CodexMessage["role"]) {
  if (role === "assistant") return tabMeta[activeTab.value].sourceLabel;
  if (role === "tool") return "工具";
  if (role === "system") return "系统";
  return "用户";
}

function makeSample(id: string, title: string, updatedAt: number, count: number, tools: string[]): CodexSession {
  const messages = Array.from({ length: count }, (_, index) => ({
    role: (index % 2 === 0 ? "user" : "assistant") as CodexMessage["role"],
    text: index % 2 === 0 ? `${title}：请分析当前项目并给出实现。` : "已经完成分析，并按现有结构做了调整。",
    timestamp: new Date(updatedAt + index * 90_000).toISOString(),
  }));
  return {
    id,
    title,
    path: `sample/${id}.jsonl`,
    size: 24_000,
    started_at: new Date(updatedAt - 20 * 60_000).toISOString(),
    updated_at: new Date(updatedAt).toISOString(),
    cwd: "E:\\JavaStudy\\work\\Aicode\\demo",
    model: tabMeta[activeTab.value].sourceLabel,
    source: "sample",
    forked_from: "",
    messages,
    tools: tools.map((name, index) => ({
      name,
      timestamp: messages[index]?.timestamp ?? new Date(updatedAt).toISOString(),
      call_id: `call_${index}`,
    })),
    role_counts: {
      user: messages.filter((m) => m.role === "user").length,
      assistant: messages.filter((m) => m.role === "assistant").length,
    },
    keywords: [
      { word: "demo", count: 2 },
      { word: "sample", count: 1 },
    ],
    total_tokens: 1500,
    documents: [],
  };
}
</script>

<template>
  <div class="app-layout">
    <nav class="sidebar">
      <div class="sidebar-brand">
        <div class="logo-icon">
          <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/><polyline points="3.27 6.96 12 12.01 20.73 6.96"/><line x1="12" y1="22.08" x2="12" y2="12"/></svg>
        </div>
        <span class="brand-text">SessionFlow</span>
      </div>
      <div class="sidebar-tabs">
        <button
          v-for="tab in (['codex', 'claude', 'opencode', 'gemini'] as const)"
          :key="tab"
          class="sidebar-tab"
          :class="{ active: activeTab === tab }"
          type="button"
          @click="activeTab = tab"
        >
          <span class="tab-icon" aria-hidden="true">{{ tabMeta[tab].icon }}</span>
          <span class="tab-label">{{ tabMeta[tab].label }}</span>
          <span v-if="src().loaded.value && activeTab === tab" class="tab-badge">✓</span>
        </button>
      </div>
      <div class="sidebar-footer">
        <button class="icon-button sidebar-theme-btn" type="button" aria-label="切换主题" title="切换主题" @click="toggleTheme">
          <svg v-if="theme === 'light'" xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/></svg>
          <svg v-else xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/></svg>
        </button>
      </div>
    </nav>

    <div class="main-area">
      <div class="shell">
        <header class="topbar">
          <div class="topbar-logo">
            <div>
              <p class="eyebrow">Developer Workspace Dashboard</p>
              <h1>{{ tabMeta[activeTab].label }} 历史会话记录 <span class="logo-version">v0.1.0</span></h1>
            </div>
          </div>
          <div class="topbar-actions">
            <div class="export-group">
              <button class="secondary-button export-btn" type="button" :disabled="!filteredSessions.length" @click="exportData">
                <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
                导出数据
              </button>
              <div class="export-format-wrapper">
                <button class="secondary-button export-format-btn" type="button" :disabled="!filteredSessions.length" @click="showExportMenu = !showExportMenu" title="选择导出格式">
                  <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
                </button>
                <div v-if="showExportMenu" class="export-menu" @mouseleave="showExportMenu = false">
                  <button type="button" :class="{ active: exportFormat === 'json' }" @click="exportFormat = 'json'; showExportMenu = false">JSON 格式</button>
                  <button type="button" :class="{ active: exportFormat === 'markdown' }" @click="exportFormat = 'markdown'; showExportMenu = false">Markdown 格式</button>
                </div>
              </div>
            </div>
          </div>
        </header>

        <main class="workspace">
          <section class="import-zone">
            <div class="import-copy">
              <div class="import-icon" aria-hidden="true">
                <svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/><polyline points="12 10 12 16 12 16"/><polyline points="9 13 12 10 15 13"/></svg>
              </div>
              <div>
                <h2>读取本机 {{ tabMeta[activeTab].label }} 历史记录</h2>
                <p>后端扫描本地 <code>{{ tabMeta[activeTab].dirDesc }}</code> 会话流。</p>
              </div>
            </div>
            <div class="import-actions">
              <input v-model="src().rootPath.value" class="path-input" type="text" :placeholder="`留空读取默认 ${tabMeta[activeTab].defaultDir}，也可填写自定义绝对路径`" aria-label="数据目录" />
              <button v-if="!loaded" class="primary-button" type="button" :disabled="loading" @click="loadHistory">
                <svg v-if="loading" class="spinner" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><circle cx="12" cy="12" r="10" stroke-dasharray="32" stroke-dashoffset="16" stroke-linecap="round"/></svg>
                <svg v-else xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-6.219-8.56"/></svg>
                {{ loading ? "正在扫描..." : "获取历史记录" }}
              </button>
              <button v-else class="secondary-button" type="button" :disabled="loading" @click="loadHistory">
                {{ loading ? "刷新中..." : "刷新" }}
              </button>
              <button class="ghost-button" type="button" @click="loadSample">
                <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>
                示例数据
              </button>
            </div>
          </section>

          <!-- Codex 历史找回面板 -->
          <section v-if="activeTab === 'codex' && loaded" class="codex-sync-panel">
            <div class="sync-header">
              <div>
                <h2>历史找回</h2>
                <p>切换 API 提供商/模型后，旧会话可能从 Codex 侧边栏消失。此功能将旧会话的 <code>model_provider</code> 和 <code>model</code> 更新为当前配置值。</p>
              </div>
            </div>

            <div v-if="syncStatus" class="sync-status-grid">
              <div class="detail-chip">
                <span class="chip-label">当前提供商</span>
                <strong class="chip-value">{{ syncStatus.config_provider || '(未设置)' }}</strong>
              </div>
              <div class="detail-chip">
                <span class="chip-label">当前模型</span>
                <strong class="chip-value">{{ syncStatus.config_model || '(未设置)' }}</strong>
              </div>
              <div class="detail-chip" :class="{ 'sync-warn': syncStatus.db_mismatched > 0 }">
                <span class="chip-label">数据库不匹配</span>
                <strong class="chip-value">{{ syncStatus.db_mismatched }} / {{ syncStatus.total_threads }}</strong>
              </div>
              <div class="detail-chip" :class="{ 'sync-warn': syncStatus.file_mismatched > 0 }">
                <span class="chip-label">文件不匹配</span>
                <strong class="chip-value">{{ syncStatus.file_mismatched }}</strong>
              </div>
              <div class="detail-chip" :class="{ 'sync-warn': syncStatus.index_missing > 0 }">
                <span class="chip-label">索引缺失</span>
                <strong class="chip-value">{{ syncStatus.index_missing }}</strong>
              </div>
            </div>

            <div v-if="syncResult" class="status-line success" style="margin-top:10px" aria-live="polite">
              <span class="status-indicator"></span>
              <span class="status-text">同步完成：数据库更新 {{ syncResult.db_updated }} 条，会话文件更新 {{ syncResult.files_updated }} 个，索引{{ syncResult.index_rebuilt ? '已重建' : '无需重建' }}。</span>
            </div>
            <div v-if="syncError" class="status-line danger" style="margin-top:10px" aria-live="polite">
              <span class="status-indicator"></span>
              <span class="status-text">{{ syncError }}</span>
            </div>

            <div class="sync-actions">
              <button class="secondary-button" type="button" :disabled="syncLoading" @click="checkSyncStatus">
                {{ syncLoading ? '检查中...' : '检查同步状态' }}
              </button>
              <button class="secondary-button" type="button" :disabled="syncLoading || !syncStatus?.needs_sync" @click="createSyncBackup">
                创建备份
              </button>
              <button class="primary-button" type="button" :disabled="syncLoading || !syncStatus?.needs_sync" @click="executeSync">
                {{ syncLoading ? '同步中...' : '执行同步' }}
              </button>
            </div>
          </section>

          <section class="status-line" :class="{ danger: statusError, success: !statusError && status.includes('已') }" aria-live="polite">
            <span class="status-indicator"></span>
            <span class="status-text">{{ status }}</span>
          </section>

          <section v-if="loaded" class="metrics-grid" aria-label="统计概览">
            <article class="metric-card">
              <div class="metric-header">
                <span class="metric-label">总会话数</span>
                <div class="metric-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                </div>
              </div>
              <strong>{{ formatNumber(summary.sessions) }}</strong>
              <span class="metric-sub">{{ summary.rangeLabel }}</span>
            </article>
            <article class="metric-card">
              <div class="metric-header">
                <span class="metric-label">消息总数</span>
                <div class="metric-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m3 21 1.9-5.7a8.5 8.5 0 1 1 3.8 3.8z"/></svg>
                </div>
              </div>
              <strong>{{ formatNumber(summary.messages) }}</strong>
              <span class="metric-sub">{{ formatNumber(summary.users) }} 用户 / {{ formatNumber(summary.assistants) }} {{ summary.sourceLabel }}</span>
            </article>
            <article class="metric-card">
              <div class="metric-header">
                <span class="metric-label">工具调用数</span>
                <div class="metric-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
                </div>
              </div>
              <strong>{{ formatNumber(summary.tools) }}</strong>
              <span class="metric-sub" :title="summary.topTool ? `最高频: ${summary.topTool[0]} (${summary.topTool[1]}次)` : '暂无工具'">
                {{ summary.topTool ? `最高频: ${summary.topTool[0]} × ${summary.topTool[1]}` : "暂无外部工具调用" }}
              </span>
            </article>
            <article class="metric-card">
              <div class="metric-header">
                <span class="metric-label">活跃天数</span>
                <div class="metric-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="4" rx="2" ry="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/></svg>
                </div>
              </div>
              <strong>{{ formatNumber(summary.activeDays) }}</strong>
              <span class="metric-sub" :title="summary.peakDay ? `峰值: ${summary.peakDay[0]} (${summary.peakDay[1]}次)` : '峰值日等待计算'">
                {{ summary.peakDay ? `峰值: ${summary.peakDay[0]} × ${summary.peakDay[1]}` : "峰值日等待计算" }}
              </span>
            </article>
            <article class="metric-card">
              <div class="metric-header">
                <span class="metric-label">总产出文档</span>
                <div class="metric-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                </div>
              </div>
              <strong>{{ formatNumber(summary.documents) }}</strong>
              <span class="metric-sub">Artifacts & 源文件</span>
            </article>
            <article class="metric-card">
              <div class="metric-header">
                <span class="metric-label">Token 消耗</span>
                <div class="metric-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v20M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/></svg>
                </div>
              </div>
              <strong>{{ summary.totalTokens > 0 ? formatTokens(summary.totalTokens) : "未记录" }}</strong>
              <span class="metric-sub">{{ summary.sessionsWithTokens }} / {{ summary.sessions }} 个会话有记录</span>
            </article>
          </section>

          <section v-if="loaded" class="visual-grid">
            <article class="panel panel-wide">
              <div class="panel-header">
                <div>
                  <h2>每日活跃</h2>
                  <p>按会话更新时间排布的会话频度</p>
                </div>
                <div class="select-wrapper">
                  <select v-model="src().range.value" aria-label="日期范围">
                    <option value="all">全部会话</option>
                    <option value="7">最近 7 天</option>
                    <option value="30">最近 30 天</option>
                    <option value="90">最近 90 天</option>
                  </select>
                </div>
              </div>
              <div class="bar-chart">
                <div v-if="!dayEntries.length" class="is-empty">
                  <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>
                  <span>读取历史后显示每日活跃度</span>
                </div>
                <div v-for="entry in dayEntries" :key="entry.day" class="bar" :title="`${entry.day}: ${entry.count} 个会话`">
                  <span class="bar-fill" :style="{ height: `${Math.max(3, (entry.count / maxDayCount) * 100)}%` }"></span>
                  <span class="bar-label">{{ entry.day.slice(5) }}</span>
                </div>
              </div>
            </article>

            <article class="panel">
              <div class="panel-header">
                <div>
                  <h2>角色占比</h2>
                  <p>用户、{{ tabMeta[activeTab].sourceLabel }}、工具输出</p>
                </div>
              </div>
              <div class="donut-wrap">
                <div class="donut-container">
                  <div class="donut" :style="{ background: roleDonut }" aria-label="角色占比图"></div>
                  <div class="donut-inner">
                    <span class="donut-total">{{ formatNumber(roleTotal) }}</span>
                    <span class="donut-total-label">消息</span>
                  </div>
                </div>
                <div class="legend">
                  <div v-if="!roleEntries.length" class="is-empty">
                    <span>暂无角色比例数据</span>
                  </div>
                  <div v-for="entry in roleEntries" :key="entry.key" class="legend-item">
                    <div class="legend-left">
                      <span class="swatch" :style="{ background: roleColors[entry.key] }"></span>
                      <span>{{ entry.label }}</span>
                    </div>
                    <strong>{{ entry.count }}</strong>
                  </div>
                </div>
              </div>
            </article>

            <article class="panel">
              <div class="panel-header">
                <div>
                  <h2>高频工具</h2>
                  <p>Top 10 工具调用次数分布</p>
                </div>
              </div>
              <div class="tool-list">
                <div v-if="!toolEntries.length" class="is-empty">
                  <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>
                  <span>暂无工具调用历史</span>
                </div>
                <div v-for="[name, count] in toolEntries" :key="name" class="tool-row">
                  <span class="tool-name" :title="name">{{ name }}</span>
                  <span class="tool-meter" aria-hidden="true">
                    <span :style="{ width: `${(count / maxToolCount) * 100}%` }"></span>
                  </span>
                  <strong class="tool-count">{{ count }}</strong>
                </div>
              </div>
            </article>

            <article class="panel panel-wide">
              <div class="panel-header">
                <div>
                  <h2>时段热力</h2>
                  <p>会话分布的24小时集中度</p>
                </div>
              </div>
              <div class="hour-heatmap-wrap">
                <div class="hour-heatmap">
                  <div
                    v-for="entry in hourEntries"
                    :key="entry.hour"
                    class="hour-cell"
                    :title="`${String(entry.hour).padStart(2, '0')}:00，${entry.count} 个会话`"
                    :style="{ background: entry.intensity ? `color-mix(in srgb, var(--accent) ${entry.intensity}%, var(--surface-soft))` : 'var(--surface-card)' }"
                  >
                    {{ String(entry.hour).padStart(2, '0') }}
                  </div>
                </div>
              </div>
            </article>
          </section>

          <section v-if="loaded" class="content-grid">
            <aside class="panel sessions-panel">
              <div class="panel-header compact">
                <div>
                  <h2>会话列表</h2>
                  <p>共加载 {{ filteredSessions.length }} 条记录</p>
                </div>
              </div>
              <div class="filter-stack">
                <div class="search-input-wrapper">
                  <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="search-icon"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/></svg>
                  <input v-model="src().query.value" type="search" placeholder="过滤会话标题、目录或消息内容..." aria-label="搜索会话" />
                </div>
                <div class="control-row">
                  <div class="select-wrapper">
                    <select v-model="src().sortMode.value" aria-label="排序方式">
                      <option value="updated-desc">最近更新</option>
                      <option value="updated-asc">最早更新</option>
                      <option value="messages-desc">最多消息</option>
                      <option value="tools-desc">最多工具</option>
                    </select>
                  </div>
                  <div class="select-wrapper">
                    <select v-model="src().docFilter.value" aria-label="文档筛选">
                      <option value="all">全部会话</option>
                      <option value="with-docs">仅看有产出</option>
                      <option value="no-docs">无产出文档</option>
                    </select>
                  </div>
                  <label class="toggle-row">
                    <input v-model="showContent" type="checkbox" class="custom-checkbox" />
                    <span>预览内容</span>
                  </label>
                </div>
              </div>
              <transition-group name="list" tag="div" class="session-list">
                <div v-if="!filteredSessions.length" class="is-empty">
                  <span>没有找到任何匹配会话</span>
                </div>
                <button
                  v-for="item in filteredSessions"
                  :key="item.id"
                  class="session-item"
                  :class="{ active: item.id === src().selectedId.value }"
                  type="button"
                  @click="src().selectedId.value = item.id"
                >
                  <div class="session-item-body">
                    <span class="session-title">{{ item.title || "未命名会话" }}</span>
                    <span class="session-meta">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
                      <span>{{ formatDateTime(item.updated_at) }}</span>
                      <span v-if="relativeTimeMap[item.id]" class="relative-time">{{ relativeTimeMap[item.id] }}</span>
                      <span class="dot-divider">·</span>
                      <span>{{ item.messages.length }} 消息</span>
                      <span v-if="item.tools.length" class="dot-divider">·</span>
                      <span v-if="item.tools.length">{{ item.tools.length }} 工具</span>
                      <span v-if="item.documents && item.documents.length" class="dot-divider">·</span>
                      <span v-if="item.documents && item.documents.length" class="has-doc-badge" title="包含产出文档">
                        <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                      </span>
                    </span>
                    <span class="session-bars">
                      <span class="mini-bar-msg" :style="{ width: `${Math.min(100, Math.max(12, item.messages.length * 3))}%` }"></span>
                      <span v-if="item.tools.length" class="mini-bar-tool" :style="{ width: `${Math.min(100, Math.max(8, item.tools.length * 8))}%` }"></span>
                    </span>
                  </div>
                </button>
              </transition-group>
            </aside>

            <section class="panel detail-panel">
              <div v-if="!selectedSession" class="detail-empty">
                <div class="empty-graphic">
                  <svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M21 9H3M21 15H3M12 3v18"/></svg>
                </div>
                <h2>请选择一个会话查看详情</h2>
                <p>载入会话数据后，这里将呈现会话中每一轮对话的详细内容、工具调用参数以及统计参数。</p>
              </div>
              <div v-else class="detail-content">
                <div class="detail-heading">
                  <div class="detail-title-area">
                    <div class="detail-time-tag">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
                      <span>会话更新于 {{ formatDateTime(selectedSession.updated_at) }}</span>
                    </div>
                    <h2>{{ selectedSession.title || "未命名会话" }}</h2>
                  </div>
                  <div class="detail-actions">
                    <button class="restore-btn cli-btn" @click="restoreSession" title="通过 CLI 命令行精确恢复会话">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/></svg>
                      CLI 恢复
                    </button>
                    <button v-if="activeTab !== 'codex'" class="restore-btn client-btn" @click="restoreViaClient" title="通过桌面客户端或 VS Code 打开项目">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/></svg>
                      客户端恢复
                    </button>
                    <button v-else class="restore-btn client-btn sync-trigger-btn" @click="executeSync" :disabled="syncLoading || !syncStatus?.needs_sync" title="将旧会话的提供商/模型更新为当前配置，使其重新出现在 Codex 侧边栏">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>
                      {{ syncLoading ? '同步中...' : (syncStatus && !syncStatus.needs_sync ? '已同步' : '历史同步') }}
                    </button>
                    <button v-if="selectedSession.cwd" class="open-dir-btn" @click="openDir(selectedSession.cwd)" title="在资源管理器中打开项目目录">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                      打开目录
                    </button>
                    <span class="badge">
                      <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/></svg>
                      <span>{{ selectedSession.model || tabMeta[activeTab].sourceLabel }}</span>
                    </span>
                  </div>
                </div>

                <div class="detail-meta">
                  <div v-for="[label, value] in detailMeta" :key="label" class="detail-chip" :class="{ 'chip-long': label === '工作目录' || label === '文件' }">
                    <span class="chip-label">{{ label }}</span>
                    <strong class="chip-value" :title="value">{{ value }}</strong>
                  </div>
                </div>

                <div class="keyword-cloud">
                  <div v-if="!selectedSession.keywords.length" class="is-empty-inline">暂无分析关键词</div>
                  <span v-for="keyword in selectedSession.keywords.slice(0, 14)" :key="keyword.word" class="keyword">
                    <span class="keyword-hash">#</span>
                    <span class="keyword-text">{{ keyword.word }}</span>
                    <span v-if="keyword.count > 1" class="keyword-count">{{ keyword.count }}</span>
                  </span>
                </div>

                <div v-if="selectedSession.documents && selectedSession.documents.length > 0" class="documents-section">
                  <h3 class="section-title">
                    <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                    产出文档 ({{ selectedSession.documents.length }})
                  </h3>
                  <div class="documents-grid">
                    <div v-for="doc in selectedSession.documents" :key="doc.path" class="document-card" @click="openDoc(doc.path)" title="点击打开文件">
                      <div class="doc-icon" :class="{ 'is-artifact': doc.doc_type.includes('Artifact') }">
                        <svg v-if="doc.doc_type.includes('Artifact')" xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>
                        <svg v-else xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
                      </div>
                      <div class="doc-info">
                        <div class="doc-path" :title="doc.path">{{ doc.path.split(/[\\/]/).pop() }}</div>
                        <div class="doc-meta">
                          <span class="doc-badge">{{ doc.doc_type }}</span>
                          <span class="doc-edits">{{ doc.action === 'create' ? '新建' : '编辑' }} {{ doc.edits }} 次</span>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>

                <div class="timeline-controls" v-if="selectedSession && showContent">
                  <h3 class="section-title timeline-title">
                    <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                    消息记录
                  </h3>
                  <div class="timeline-filters">
                    <div class="select-wrapper">
                      <select v-model="messageRoleFilter" aria-label="角色筛选">
                        <option value="all">全部角色</option>
                        <option value="assistant">仅 {{ tabMeta[activeTab].sourceLabel }}</option>
                        <option value="user">仅用户</option>
                      </select>
                    </div>
                    <div class="select-wrapper">
                      <select v-model="messageLimit" aria-label="预览条数">
                        <option :value="10">10 条</option>
                        <option :value="50">50 条</option>
                        <option :value="100">100 条</option>
                        <option :value="500">500 条</option>
                        <option :value="5000">5000 条</option>
                      </select>
                    </div>
                  </div>
                </div>

                <div class="message-timeline">
                  <div v-if="!showContent" class="is-empty">
                    <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12h20M2 12a10 10 0 0 1 20 0Z"/></svg>
                    <span>已关闭内容预览</span>
                  </div>
                  <article
                    v-for="message in showContent ? (expandAllMessages ? filteredTimelineMessages : filteredTimelineMessages.slice(0, messageLimit)) : []"
                    :key="`${message.timestamp}-${message.role}-${message.text.slice(0, 20)}`"
                    class="message-card"
                    :class="`role-${message.role}`"
                  >
                    <div class="message-head">
                      <span class="role-icon-wrapper" :style="{ color: roleColors[message.role] }">
                        <svg v-if="message.role === 'user'" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>
                        <svg v-else-if="message.role === 'assistant'" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2M20 14h2M15 13v2M9 13v2M12 8V4H8"/></svg>
                        <svg v-else-if="message.role === 'tool'" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/></svg>
                        <svg v-else xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
                      </span>
                      <strong class="role-name">{{ roleLabel(message.role) }}</strong>
                      <time class="message-time">{{ formatDateTime(message.timestamp) }}</time>
                    </div>
                    <div class="message-body-wrap">
                      <pre class="message-text">{{ message.text }}</pre>
                    </div>
                  </article>
                  <div v-if="showContent && !expandAllMessages && filteredTimelineMessages.length > messageLimit" class="is-empty-footer" @click="expandAllMessages = true" >
                    <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M5 12h14"/></svg>
                    <span>点击展开其余的 {{ filteredTimelineMessages.length - messageLimit }} 条消息</span>
                  </div>
                </div>
              </div>
            </section>
          </section>
        </main>
      </div>
    </div>
  </div>
</template>

<style scoped>
.list-enter-active,
.list-leave-active {
  transition: all 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}
.list-enter-from,
.list-leave-to {
  opacity: 0;
  transform: translateX(-20px) scale(0.98);
}
.list-leave-active {
  position: absolute;
}
</style>
