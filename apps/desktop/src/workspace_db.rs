//! Local SQLite persistence for Agent Projects, Tasks DB, and `llama.cpp` runtime tuning (`agent_workspace.db`).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
}

impl TaskStatus {
    pub const ALL: [Self; 4] = [Self::Todo, Self::InProgress, Self::Blocked, Self::Done];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Todo,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Todo => "Queued",
            Self::InProgress => "Running",
            Self::Blocked => "Blocked",
            Self::Done => "Done",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Todo => Self::InProgress,
            Self::InProgress => Self::Done,
            Self::Blocked => Self::InProgress,
            Self::Done => Self::Todo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskPriority {
    P0,
    P1,
    P2,
    P3,
}

impl TaskPriority {
    pub const ALL: [Self; 4] = [Self::P0, Self::P1, Self::P2, Self::P3];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "P0" => Self::P0,
            "P2" => Self::P2,
            "P3" => Self::P3,
            _ => Self::P1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFilter {
    All,
    Daily,
    Code,
    Open,
    InProgress,
    Done,
}

impl TaskFilter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Daily,
        Self::Code,
        Self::Open,
        Self::InProgress,
        Self::Done,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Daily => "Daily",
            Self::Code => "Code",
            Self::Open => "Open",
            Self::InProgress => "Running",
            Self::Done => "Done",
        }
    }
}

/// Logical workstream classification for Tasks, Daily Logs & Code Runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaskKind {
    #[default]
    General,
    Daily,
    Code,
    Review,
}

impl TaskKind {
    pub const ALL: [Self; 4] = [Self::General, Self::Daily, Self::Code, Self::Review];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Daily => "daily",
            Self::Code => "code",
            Self::Review => "review",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "daily" => Self::Daily,
            "code" => Self::Code,
            "review" => Self::Review,
            _ => Self::General,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Daily => "Daily",
            Self::Code => "Code",
            Self::Review => "Review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KvCacheType {
    F16,
    Q8_0,
    Q4_0,
}

impl KvCacheType {
    pub const ALL: [Self; 3] = [Self::F16, Self::Q8_0, Self::Q4_0];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::F16 => "f16",
            Self::Q8_0 => "q8_0",
            Self::Q4_0 => "q4_0",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::F16 => "FP16 (Ref)",
            Self::Q8_0 => "Q8_0 (2x)",
            Self::Q4_0 => "Q4_0 (4x)",
        }
    }

    pub fn bits_per_elem(self) -> f32 {
        match self {
            Self::F16 => 16.0,
            Self::Q8_0 => 8.5,
            Self::Q4_0 => 4.5,
        }
    }
}

/// Complete `llama.cpp` runtime & sampling matrix persisted in SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaRuntimeConfig {
    pub n_gpu_layers: u32,
    pub n_threads: u32,
    pub n_batch: u32,
    pub ctx_size: u32,
    pub flash_attention: bool,
    pub mmap: bool,
    pub mlock: bool,
    pub kv_offload: bool,
    pub kv_cache_type: KvCacheType,
    pub temperature_x100: u32,
    pub top_p_x100: u32,
    pub top_k: u32,
    pub repeat_penalty_x100: u32,
    pub seed: u32,
    pub main_gpu: u32,
}

impl Default for LlamaRuntimeConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(8)
            .clamp(2, 64);
        Self {
            n_gpu_layers: 32,
            n_threads: (cores / 2).max(2),
            n_batch: 512,
            ctx_size: 4096,
            flash_attention: true,
            mmap: true,
            mlock: false,
            kv_offload: true,
            kv_cache_type: KvCacheType::Q8_0,
            temperature_x100: 70,
            top_p_x100: 95,
            top_k: 40,
            repeat_penalty_x100: 110,
            seed: 0,
            main_gpu: 0,
        }
    }
}

impl LlamaRuntimeConfig {
    /// Approximate KV cache footprint in MiB assuming a typical 32-layer GQA transformer (`~8 KV heads x 128`).
    pub fn estimated_kv_mib(&self) -> f32 {
        let bytes_per_tok = 2.0 * 32.0 * 1024.0 * (self.kv_cache_type.bits_per_elem() / 8.0);
        let raw_mib = (self.ctx_size as f32 * bytes_per_tok) / (1024.0 * 1024.0);
        if self.flash_attention {
            raw_mib * 0.82
        } else {
            raw_mib
        }
    }

    /// Generate the exact `llama.cpp` CLI / worker runtime command preview.
    pub fn cli_preview(&self, model_alias: Option<&str>) -> String {
        let model = model_alias.unwrap_or("models/model.gguf");
        let fa = if self.flash_attention {
            " --flash-attn"
        } else {
            ""
        };
        let mlock = if self.mlock { " --mlock" } else { "" };
        let no_mmap = if !self.mmap { " --no-mmap" } else { "" };
        let no_kv_off = if !self.kv_offload {
            " --no-kv-offload"
        } else {
            ""
        };
        format!(
            "llama-cli -m \"{model}\" -ngl {} -t {} -b {} -c {}{fa}{mlock}{no_mmap}{no_kv_off} \
             --cache-type-k {} --cache-type-v {} --temp {:.2} --top-p {:.2} --top-k {} \
             --repeat-penalty {:.2} --seed {} -mg {}",
            self.n_gpu_layers,
            self.n_threads,
            self.n_batch,
            self.ctx_size,
            self.kv_cache_type.as_str(),
            self.kv_cache_type.as_str(),
            self.temperature_x100 as f32 / 100.0,
            self.top_p_x100 as f32 / 100.0,
            self.top_k,
            self.repeat_penalty_x100 as f32 / 100.0,
            self.seed,
            self.main_gpu,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProject {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub description: String,
    pub system_prompt: String,
    pub accent_tag: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTask {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub notes: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub task_kind: TaskKind,
    pub assignee: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

/// Functional Agent specialty selectable per turn in Chat, Code, Daily & Tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentKind {
    #[default]
    General,
    Coder,
    Planner,
    Researcher,
    Reviewer,
}

impl AgentKind {
    pub const ALL: [Self; 5] = [
        Self::General,
        Self::Coder,
        Self::Planner,
        Self::Researcher,
        Self::Reviewer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Coder => "Coder",
            Self::Planner => "Daily/Plan",
            Self::Researcher => "Research",
            Self::Reviewer => "Review",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Coder => "coder",
            Self::Planner => "planner",
            Self::Researcher => "researcher",
            Self::Reviewer => "reviewer",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "coder" => Self::Coder,
            "planner" => Self::Planner,
            "researcher" => Self::Researcher,
            "reviewer" => Self::Reviewer,
            _ => Self::General,
        }
    }

    pub fn directive(self) -> &'static str {
        match self {
            Self::General => "Assist clearly across tasks, workspace questions, and analysis.",
            Self::Coder => "Act as a senior systems engineer: cite exact file paths, write clean production code/tests, and avoid duplication.",
            Self::Planner => "Structure output into prioritized daily execution checklists, blockers, and concrete next steps.",
            Self::Researcher => "Synthesize grounded evidence from local workspace context and mark unverified assumptions explicitly.",
            Self::Reviewer => "Audit strictly for bugs, edge cases, trust boundaries, and regressions with minimal diffs.",
        }
    }
}

/// Reasoning effort / thinking depth budget selectable per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReasoningEffort {
    Low,
    #[default]
    Medium,
    High,
    Max,
}

impl ReasoningEffort {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Max];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Med",
            Self::High => "High",
            Self::Max => "Max",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "low" => Self::Low,
            "high" => Self::High,
            "max" => Self::Max,
            _ => Self::Medium,
        }
    }

    pub fn thinking_reserve_tokens(self) -> usize {
        match self {
            Self::Low => 128,
            Self::Medium => 384,
            Self::High => 896,
            Self::Max => 1600,
        }
    }

    pub fn directive(self) -> &'static str {
        match self {
            Self::Low => "Reasoning effort LOW: answer directly and fast without long deliberation.",
            Self::Medium => "Reasoning effort MEDIUM: verify key constraints concisely before answering.",
            Self::High => "Reasoning effort HIGH: inspect edge cases thoroughly in <think>...</think> before answering.",
            Self::Max => "Reasoning effort MAX: perform exhaustive multi-pass verification, failure-mode checks, and invariants inside <think>...</think>.",
        }
    }
}

/// Execution / model access route: air-gapped local GGUF runtime vs explicit user-configured API endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExecutionAccessMode {
    #[default]
    Local,
    Api,
}

impl ExecutionAccessMode {
    pub const ALL: [Self; 2] = [Self::Local, Self::Api];

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Api => "API",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Api => "api",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "api" => Self::Api,
            _ => Self::Local,
        }
    }
}

/// OpenAI-compatible remote/local-gateway endpoint metadata stored for `ExecutionAccessMode::Api`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiEndpointConfig {
    pub base_url: String,
    pub model_name: String,
    pub api_key_env: String,
}

impl Default for ApiEndpointConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            model_name: "gpt-4o-mini".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
        }
    }
}

/// Workspace folder trust & access capability tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FolderAccessLevel {
    Deny,
    #[default]
    ReadOnly,
    ReadWrite,
    Trusted,
}

impl FolderAccessLevel {
    pub const ALL: [Self; 4] = [Self::Deny, Self::ReadOnly, Self::ReadWrite, Self::Trusted];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
            Self::Trusted => "trusted",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "deny" => Self::Deny,
            "read_write" => Self::ReadWrite,
            "trusted" => Self::Trusted,
            _ => Self::ReadOnly,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Deny => "DENY",
            Self::ReadOnly => "READ-ONLY",
            Self::ReadWrite => "READ/WRITE",
            Self::Trusted => "TRUSTED",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::ReadOnly => Self::ReadWrite,
            Self::ReadWrite => Self::Trusted,
            Self::Trusted => Self::Deny,
            Self::Deny => Self::ReadOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedFolderEntry {
    pub id: i64,
    pub project_id: i64,
    pub path: String,
    pub access_level: FolderAccessLevel,
    pub created_at_ms: u64,
}

/// Scoped environment variable stored in SQLite (`project_id == 0` denotes Global scope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVarEntry {
    pub id: i64,
    pub project_id: i64,
    pub env_key: String,
    pub env_val: String,
    pub is_secret: bool,
    pub enabled: bool,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentPersona {
    #[default]
    Balanced,
    Concise,
    DeepArchitect,
    StrictAuditor,
}

impl AgentPersona {
    pub const ALL: [Self; 4] = [
        Self::Balanced,
        Self::Concise,
        Self::DeepArchitect,
        Self::StrictAuditor,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Concise => "Concise",
            Self::DeepArchitect => "Architect",
            Self::StrictAuditor => "Auditor",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Concise => "concise",
            Self::DeepArchitect => "architect",
            Self::StrictAuditor => "auditor",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "concise" => Self::Concise,
            "architect" => Self::DeepArchitect,
            "auditor" => Self::StrictAuditor,
            _ => Self::Balanced,
        }
    }

    pub fn directive(self) -> &'static str {
        match self {
            Self::Balanced => "",
            Self::Concise => "Respond concisely with crisp bullet points and zero filler.",
            Self::DeepArchitect => {
                "Explain trade-offs, system boundaries, and failure modes step by step inside <think>...</think> before the final answer."
            }
            Self::StrictAuditor => {
                "Audit strictly for security, determinism, and verifiable file citations. Flag every unverified assumption."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedChatRow {
    pub msg_id: String,
    pub is_user: bool,
    pub author: String,
    pub badge: String,
    pub raw_text: String,
    pub outcome_sha256: Option<String>,
    pub outcome_verified: bool,
    pub citations: Vec<String>,
    pub receipt_count: usize,
}

/// Real SQLite workspace store (`agent_workspace.db`) backing Projects, Agent Tasks DB, and `llama.cpp` config.
pub struct WorkspaceDb {
    conn: Connection,
}

impl WorkspaceDb {
    pub fn open_or_memory(data_dir: &Path, default_root: &str) -> Self {
        let db_path = data_dir.join("agent_workspace.db");
        let conn = Connection::open(&db_path)
            .or_else(|_| Connection::open_in_memory())
            .expect("sqlite open fallback in-memory");
        let _ = conn.busy_timeout(Duration::from_millis(2_000));
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "foreign_keys", "ON");

        let db = Self { conn };
        db.ensure_schema();
        db.seed_defaults_if_empty(default_root);
        db
    }

    fn ensure_schema(&self) {
        let _ = self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS workspace_projects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                root_path TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                system_prompt TEXT NOT NULL DEFAULT '',
                accent_tag TEXT NOT NULL DEFAULT 'CORE',
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agent_tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL REFERENCES workspace_projects(id) ON DELETE CASCADE,
                title TEXT NOT NULL,
                notes TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'todo',
                priority TEXT NOT NULL DEFAULT 'P1',
                task_kind TEXT NOT NULL DEFAULT 'general',
                assignee TEXT NOT NULL DEFAULT 'Lora Agent',
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                completed_at_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_agent_tasks_project ON agent_tasks(project_id, status);
            CREATE TABLE IF NOT EXISTS workspace_kv (
                key TEXT PRIMARY KEY,
                val TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS trusted_folders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL REFERENCES workspace_projects(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                access_level TEXT NOT NULL DEFAULT 'read_only',
                created_at_ms INTEGER NOT NULL,
                UNIQUE(project_id, path)
            );
            CREATE TABLE IF NOT EXISTS workspace_env_vars (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL DEFAULT 0,
                env_key TEXT NOT NULL,
                env_val TEXT NOT NULL,
                is_secret INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at_ms INTEGER NOT NULL,
                UNIQUE(project_id, env_key)
            );
            CREATE TABLE IF NOT EXISTS chat_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL REFERENCES workspace_projects(id) ON DELETE CASCADE,
                msg_id TEXT NOT NULL,
                is_user INTEGER NOT NULL,
                author TEXT NOT NULL,
                badge TEXT NOT NULL,
                raw_text TEXT NOT NULL,
                outcome_sha256 TEXT,
                outcome_verified INTEGER NOT NULL DEFAULT 0,
                citations_json TEXT NOT NULL DEFAULT '[]',
                receipt_count INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chat_history_project ON chat_history(project_id, id);",
        );
        self.ensure_task_columns();
    }

    fn ensure_task_columns(&self) {
        let mut has_kind = false;
        let mut has_completed = false;
        if let Ok(mut stmt) = self.conn.prepare("PRAGMA table_info(agent_tasks)") {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
                for name in rows.filter_map(Result::ok) {
                    if name == "task_kind" {
                        has_kind = true;
                    } else if name == "completed_at_ms" {
                        has_completed = true;
                    }
                }
            }
        }
        if !has_kind {
            let _ = self.conn.execute(
                "ALTER TABLE agent_tasks ADD COLUMN task_kind TEXT NOT NULL DEFAULT 'general'",
                [],
            );
        }
        if !has_completed {
            let _ = self.conn.execute(
                "ALTER TABLE agent_tasks ADD COLUMN completed_at_ms INTEGER",
                [],
            );
        }
    }

    fn seed_defaults_if_empty(&self, default_root: &str) {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM workspace_projects", [], |r| r.get(0))
            .unwrap_or(0);
        if count > 0 {
            return;
        }
        let now = epoch_ms();
        let _ = self.conn.execute(
            "INSERT INTO workspace_projects (name, root_path, description, system_prompt, accent_tag, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "Lora Core",
                default_root,
                "Primary air-gapped Rust workspace & agent governance hub.",
                "Operate strictly offline with read-only scoped citations and explicit user approvals.",
                "CORE",
                now as i64
            ],
        );
        let project_id = self.conn.last_insert_rowid();
        let _ = self.upsert_trusted_folder(project_id, default_root, FolderAccessLevel::ReadOnly);
        let seeds = [
            (
                "Verify local GGUF artifact & SHA-256 digest",
                "Import a quantized model in Models and inspect context headroom.",
                TaskStatus::Todo,
                TaskPriority::P0,
                TaskKind::General,
                "Model Ops",
                None,
            ),
            (
                "Tune llama.cpp KV cache & thread budget",
                "Adjust GPU layers, Flash Attention, and batch size for host hardware.",
                TaskStatus::InProgress,
                TaskPriority::P1,
                TaskKind::Code,
                "Llama Engine",
                None,
            ),
            (
                "Review workspace read-only trust boundaries",
                "Verify scope root and preview default-Deny consent modal.",
                TaskStatus::Done,
                TaskPriority::P2,
                TaskKind::Daily,
                "Security Gate",
                Some(now as i64),
            ),
        ];
        for (title, notes, status, prio, kind, assignee, completed_at) in seeds {
            let _ = self.conn.execute(
                "INSERT INTO agent_tasks (project_id, title, notes, status, priority, task_kind, assignee, created_at_ms, updated_at_ms, completed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    project_id,
                    title,
                    notes,
                    status.as_str(),
                    prio.as_str(),
                    kind.as_str(),
                    assignee,
                    now as i64,
                    now as i64,
                    completed_at,
                ],
            );
        }
    }

    pub fn list_projects(&self) -> Vec<WorkspaceProject> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT id, name, root_path, description, system_prompt, accent_tag, created_at_ms
             FROM workspace_projects ORDER BY id ASC",
        ) else {
            return vec![];
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok(WorkspaceProject {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                description: row.get(3)?,
                system_prompt: row.get(4)?,
                accent_tag: row.get(5)?,
                created_at_ms: row.get::<_, i64>(6)? as u64,
            })
        }) else {
            return vec![];
        };
        rows.filter_map(Result::ok).collect()
    }

    pub fn create_project(
        &self,
        name: &str,
        root_path: &str,
        description: &str,
        system_prompt: &str,
        accent_tag: &str,
    ) -> Option<i64> {
        let clean_name = name.trim();
        if clean_name.is_empty() {
            return None;
        }
        let now = epoch_ms() as i64;
        self.conn
            .execute(
                "INSERT INTO workspace_projects (name, root_path, description, system_prompt, accent_tag, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    clean_name,
                    root_path.trim(),
                    description.trim(),
                    system_prompt.trim(),
                    if accent_tag.trim().is_empty() { "PROJ" } else { accent_tag.trim() },
                    now,
                ],
            )
            .ok()?;
        let pid = self.conn.last_insert_rowid();
        if !root_path.trim().is_empty() {
            let _ = self.upsert_trusted_folder(pid, root_path.trim(), FolderAccessLevel::ReadOnly);
        }
        Some(pid)
    }

    pub fn delete_project(&self, project_id: i64) -> bool {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM workspace_projects", [], |r| r.get(0))
            .unwrap_or(0);
        if count <= 1 {
            return false;
        }
        self.conn
            .execute(
                "DELETE FROM workspace_projects WHERE id = ?1",
                params![project_id],
            )
            .is_ok_and(|n| n > 0)
    }

    pub fn list_tasks(&self, project_id: Option<i64>) -> Vec<AgentTask> {
        let sql = match project_id {
            Some(_) => {
                "SELECT id, project_id, title, notes, status, priority, task_kind, assignee, created_at_ms, updated_at_ms, completed_at_ms
                 FROM agent_tasks WHERE project_id = ?1
                 ORDER BY CASE status WHEN 'in_progress' THEN 0 WHEN 'todo' THEN 1 WHEN 'blocked' THEN 2 ELSE 3 END, id DESC"
            }
            None => {
                "SELECT id, project_id, title, notes, status, priority, task_kind, assignee, created_at_ms, updated_at_ms, completed_at_ms
                 FROM agent_tasks
                 ORDER BY CASE status WHEN 'in_progress' THEN 0 WHEN 'todo' THEN 1 WHEN 'blocked' THEN 2 ELSE 3 END, id DESC"
            }
        };
        let Ok(mut stmt) = self.conn.prepare(sql) else {
            return vec![];
        };
        let mapper = |row: &rusqlite::Row<'_>| {
            let status_s: String = row.get(4)?;
            let prio_s: String = row.get(5)?;
            let kind_s: String = row.get(6)?;
            Ok(AgentTask {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                notes: row.get(3)?,
                status: TaskStatus::parse(&status_s),
                priority: TaskPriority::parse(&prio_s),
                task_kind: TaskKind::parse(&kind_s),
                assignee: row.get(7)?,
                created_at_ms: row.get::<_, i64>(8)? as u64,
                updated_at_ms: row.get::<_, i64>(9)? as u64,
                completed_at_ms: row.get::<_, Option<i64>>(10)?.map(|v| v.max(0) as u64),
            })
        };
        if let Some(pid) = project_id {
            stmt.query_map(params![pid], mapper)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        } else {
            stmt.query_map([], mapper)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        }
    }

    pub fn create_task(
        &self,
        project_id: i64,
        title: &str,
        notes: &str,
        priority: TaskPriority,
        assignee: &str,
    ) -> Option<i64> {
        self.create_task_with_kind(
            project_id,
            title,
            notes,
            priority,
            TaskKind::General,
            assignee,
        )
    }

    pub fn create_task_with_kind(
        &self,
        project_id: i64,
        title: &str,
        notes: &str,
        priority: TaskPriority,
        task_kind: TaskKind,
        assignee: &str,
    ) -> Option<i64> {
        let clean_title = title.trim();
        if clean_title.is_empty() {
            return None;
        }
        let now = epoch_ms() as i64;
        self.conn
            .execute(
                "INSERT INTO agent_tasks (project_id, title, notes, status, priority, task_kind, assignee, created_at_ms, updated_at_ms, completed_at_ms)
                 VALUES (?1, ?2, ?3, 'todo', ?4, ?5, ?6, ?7, ?8, NULL)",
                params![
                    project_id,
                    clean_title,
                    notes.trim(),
                    priority.as_str(),
                    task_kind.as_str(),
                    if assignee.trim().is_empty() {
                        "Lora Agent"
                    } else {
                        assignee.trim()
                    },
                    now,
                    now,
                ],
            )
            .ok()?;
        Some(self.conn.last_insert_rowid())
    }

    pub fn update_task_status(&self, task_id: i64, status: TaskStatus) -> bool {
        let now = epoch_ms() as i64;
        let completed = if status == TaskStatus::Done {
            Some(now)
        } else {
            None
        };
        self.conn
            .execute(
                "UPDATE agent_tasks SET status = ?1, updated_at_ms = ?2, completed_at_ms = ?3 WHERE id = ?4",
                params![status.as_str(), now, completed, task_id],
            )
            .is_ok_and(|n| n > 0)
    }

    pub fn delete_task(&self, task_id: i64) -> bool {
        self.conn
            .execute("DELETE FROM agent_tasks WHERE id = ?1", params![task_id])
            .is_ok_and(|n| n > 0)
    }

    fn load_kv(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT val FROM workspace_kv WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap_or(None)
    }

    fn save_kv(&self, key: &str, val: &str) {
        let _ = self.conn.execute(
            "INSERT INTO workspace_kv (key, val) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET val = excluded.val",
            params![key, val],
        );
    }

    pub fn load_llama_config(&self) -> LlamaRuntimeConfig {
        self.load_kv("llama_config_v1")
            .and_then(|s| serde_json::from_str::<LlamaRuntimeConfig>(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_llama_config(&self, cfg: &LlamaRuntimeConfig) {
        if let Ok(json) = serde_json::to_string(cfg) {
            self.save_kv("llama_config_v1", &json);
        }
    }

    pub fn load_active_project_id(&self) -> Option<i64> {
        self.load_kv("active_project_id")
            .and_then(|s| s.parse::<i64>().ok())
    }

    pub fn save_active_project_id(&self, id: i64) {
        self.save_kv("active_project_id", &id.to_string());
    }

    pub fn load_persona(&self) -> AgentPersona {
        self.load_kv("agent_persona_v1")
            .map(|s| AgentPersona::parse(&s))
            .unwrap_or_default()
    }

    pub fn save_persona(&self, persona: AgentPersona) {
        self.save_kv("agent_persona_v1", persona.code());
    }

    pub fn load_agent_kind(&self) -> AgentKind {
        self.load_kv("agent_kind_v1")
            .map(|s| AgentKind::parse(&s))
            .unwrap_or_default()
    }

    pub fn save_agent_kind(&self, kind: AgentKind) {
        self.save_kv("agent_kind_v1", kind.code());
    }

    pub fn load_reasoning_effort(&self) -> ReasoningEffort {
        self.load_kv("reasoning_effort_v1")
            .map(|s| ReasoningEffort::parse(&s))
            .unwrap_or_default()
    }

    pub fn save_reasoning_effort(&self, effort: ReasoningEffort) {
        self.save_kv("reasoning_effort_v1", effort.code());
    }

    pub fn load_access_mode(&self) -> ExecutionAccessMode {
        self.load_kv("access_mode_v1")
            .map(|s| ExecutionAccessMode::parse(&s))
            .unwrap_or_default()
    }

    pub fn save_access_mode(&self, mode: ExecutionAccessMode) {
        self.save_kv("access_mode_v1", mode.code());
    }

    pub fn load_api_config(&self) -> ApiEndpointConfig {
        self.load_kv("api_endpoint_cfg_v1")
            .and_then(|s| serde_json::from_str::<ApiEndpointConfig>(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_api_config(&self, cfg: &ApiEndpointConfig) {
        if let Ok(json) = serde_json::to_string(cfg) {
            self.save_kv("api_endpoint_cfg_v1", &json);
        }
    }

    pub fn list_trusted_folders(&self, project_id: Option<i64>) -> Vec<TrustedFolderEntry> {
        let sql = match project_id {
            Some(_) => {
                "SELECT id, project_id, path, access_level, created_at_ms
                 FROM trusted_folders WHERE project_id = ?1 ORDER BY id DESC"
            }
            None => {
                "SELECT id, project_id, path, access_level, created_at_ms
                 FROM trusted_folders ORDER BY id DESC"
            }
        };
        let Ok(mut stmt) = self.conn.prepare(sql) else {
            return vec![];
        };
        let mapper = |r: &rusqlite::Row<'_>| {
            let lvl: String = r.get(3)?;
            Ok(TrustedFolderEntry {
                id: r.get(0)?,
                project_id: r.get(1)?,
                path: r.get(2)?,
                access_level: FolderAccessLevel::parse(&lvl),
                created_at_ms: r.get::<_, i64>(4)? as u64,
            })
        };
        if let Some(pid) = project_id {
            stmt.query_map(params![pid], mapper)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        } else {
            stmt.query_map([], mapper)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        }
    }

    pub fn upsert_trusted_folder(
        &self,
        project_id: i64,
        path: &str,
        level: FolderAccessLevel,
    ) -> Option<i64> {
        let clean = path.trim();
        if clean.is_empty() {
            return None;
        }
        let now = epoch_ms() as i64;
        self.conn
            .execute(
                "INSERT INTO trusted_folders (project_id, path, access_level, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(project_id, path) DO UPDATE SET access_level = excluded.access_level",
                params![project_id, clean, level.as_str(), now],
            )
            .ok()?;
        Some(self.conn.last_insert_rowid())
    }

    pub fn update_trusted_folder_level(&self, folder_id: i64, level: FolderAccessLevel) -> bool {
        self.conn
            .execute(
                "UPDATE trusted_folders SET access_level = ?1 WHERE id = ?2",
                params![level.as_str(), folder_id],
            )
            .is_ok_and(|n| n > 0)
    }

    pub fn delete_trusted_folder(&self, folder_id: i64) -> bool {
        self.conn
            .execute(
                "DELETE FROM trusted_folders WHERE id = ?1",
                params![folder_id],
            )
            .is_ok_and(|n| n > 0)
    }

    pub fn folder_trust_for_path(&self, project_id: i64, path: &str) -> FolderAccessLevel {
        let norm_target = normalize_folder_match(path);
        if norm_target.is_empty() {
            return FolderAccessLevel::ReadOnly;
        }
        let folders = self.list_trusted_folders(Some(project_id));
        let mut best_len = 0usize;
        let mut best_level = None;
        for f in folders {
            let norm_root = normalize_folder_match(&f.path);
            if norm_root.is_empty() {
                continue;
            }
            if (norm_target == norm_root || norm_target.starts_with(&format!("{norm_root}/")))
                && norm_root.len() >= best_len
            {
                best_len = norm_root.len();
                best_level = Some(f.access_level);
            }
        }
        best_level.unwrap_or(FolderAccessLevel::ReadOnly)
    }

    pub fn list_env_vars(&self, project_id: Option<i64>) -> Vec<EnvVarEntry> {
        let sql = match project_id {
            Some(_) => {
                "SELECT id, project_id, env_key, env_val, is_secret, enabled, updated_at_ms
                 FROM workspace_env_vars
                 WHERE project_id = 0 OR project_id = ?1
                 ORDER BY project_id DESC, env_key ASC"
            }
            None => {
                "SELECT id, project_id, env_key, env_val, is_secret, enabled, updated_at_ms
                 FROM workspace_env_vars
                 ORDER BY project_id DESC, env_key ASC"
            }
        };
        let Ok(mut stmt) = self.conn.prepare(sql) else {
            return vec![];
        };
        let mapper = |r: &rusqlite::Row<'_>| {
            Ok(EnvVarEntry {
                id: r.get(0)?,
                project_id: r.get(1)?,
                env_key: r.get(2)?,
                env_val: r.get(3)?,
                is_secret: r.get::<_, i32>(4)? != 0,
                enabled: r.get::<_, i32>(5)? != 0,
                updated_at_ms: r.get::<_, i64>(6)? as u64,
            })
        };
        if let Some(pid) = project_id {
            stmt.query_map(params![pid], mapper)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        } else {
            stmt.query_map([], mapper)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        }
    }

    pub fn upsert_env_var(
        &self,
        project_id: i64,
        env_key: &str,
        env_val: &str,
        is_secret: bool,
    ) -> Option<i64> {
        let key = env_key.trim();
        if key.is_empty() {
            return None;
        }
        let now = epoch_ms() as i64;
        self.conn
            .execute(
                "INSERT INTO workspace_env_vars (project_id, env_key, env_val, is_secret, enabled, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)
                 ON CONFLICT(project_id, env_key) DO UPDATE SET
                   env_val = excluded.env_val,
                   is_secret = excluded.is_secret,
                   enabled = 1,
                   updated_at_ms = excluded.updated_at_ms",
                params![project_id, key, env_val, i32::from(is_secret), now],
            )
            .ok()?;
        Some(self.conn.last_insert_rowid())
    }

    pub fn toggle_env_var(&self, env_id: i64, enabled: bool) -> bool {
        let now = epoch_ms() as i64;
        self.conn
            .execute(
                "UPDATE workspace_env_vars SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![i32::from(enabled), now, env_id],
            )
            .is_ok_and(|n| n > 0)
    }

    pub fn delete_env_var(&self, env_id: i64) -> bool {
        self.conn
            .execute(
                "DELETE FROM workspace_env_vars WHERE id = ?1",
                params![env_id],
            )
            .is_ok_and(|n| n > 0)
    }

    /// Parse standard `.env` lines (`KEY=VALUE`) and persist enabled variables for `project_id` (`0` = global).
    pub fn import_dotenv_text(&self, project_id: i64, content: &str) -> usize {
        let mut count = 0usize;
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let cleaned = line.strip_prefix("export ").unwrap_or(line);
            let Some((k, v)) = cleaned.split_once('=') else {
                continue;
            };
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            let val = v.trim();
            let unquoted = if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                &val[1..val.len() - 1]
            } else {
                val
            };
            let upper = key.to_ascii_uppercase();
            let secret = upper.contains("KEY")
                || upper.contains("SECRET")
                || upper.contains("TOKEN")
                || upper.contains("PASS");
            if self
                .upsert_env_var(project_id, key, unquoted, secret)
                .is_some()
            {
                count += 1;
            }
        }
        count
    }

    /// Resolve active environment variable (`Project scope` -> `Global scope` -> OS process env).
    pub fn resolve_env_value(&self, project_id: i64, key: &str) -> Option<String> {
        let entries = self.list_env_vars(Some(project_id));
        if let Some(found) = entries
            .into_iter()
            .find(|e| e.enabled && e.env_key.eq_ignore_ascii_case(key) && !e.env_val.is_empty())
        {
            return Some(found.env_val);
        }
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }

    pub fn replace_project_chat(&self, project_id: i64, rows: &[PersistedChatRow]) {
        let _ = self.conn.execute(
            "DELETE FROM chat_history WHERE project_id = ?1",
            params![project_id],
        );
        let now = epoch_ms() as i64;
        for row in rows {
            let cit_json =
                serde_json::to_string(&row.citations).unwrap_or_else(|_| "[]".to_string());
            let _ = self.conn.execute(
                "INSERT INTO chat_history (
                    project_id, msg_id, is_user, author, badge, raw_text,
                    outcome_sha256, outcome_verified, citations_json, receipt_count, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    project_id,
                    row.msg_id,
                    i32::from(row.is_user),
                    row.author,
                    row.badge,
                    row.raw_text,
                    row.outcome_sha256,
                    i32::from(row.outcome_verified),
                    cit_json,
                    row.receipt_count as i64,
                    now,
                ],
            );
        }
    }

    pub fn load_project_chat(&self, project_id: i64) -> Vec<PersistedChatRow> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT msg_id, is_user, author, badge, raw_text, outcome_sha256, outcome_verified, citations_json, receipt_count
             FROM chat_history WHERE project_id = ?1 ORDER BY id ASC",
        ) else {
            return vec![];
        };
        let Ok(rows) = stmt.query_map(params![project_id], |r| {
            let cit_json: String = r.get(7)?;
            let citations = serde_json::from_str::<Vec<String>>(&cit_json).unwrap_or_default();
            Ok(PersistedChatRow {
                msg_id: r.get(0)?,
                is_user: r.get::<_, i32>(1)? != 0,
                author: r.get(2)?,
                badge: r.get(3)?,
                raw_text: r.get(4)?,
                outcome_sha256: r.get(5)?,
                outcome_verified: r.get::<_, i32>(6)? != 0,
                citations,
                receipt_count: r.get::<_, i64>(8)?.max(0) as usize,
            })
        }) else {
            return vec![];
        };
        rows.filter_map(Result::ok).collect()
    }
}

fn normalize_folder_match(raw: &str) -> String {
    let unified = raw.trim().replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    #[cfg(target_os = "windows")]
    {
        trimmed.to_ascii_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        trimmed.to_string()
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sqlite_projects_tasks_and_llama_config_roundtrip() {
        let tmp = TempDir::new().expect("tempdir");
        let db = WorkspaceDb::open_or_memory(tmp.path(), "/workspace");

        let projects = db.list_projects();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Lora Core");

        let initial_tasks = db.list_tasks(Some(projects[0].id));
        assert_eq!(initial_tasks.len(), 3);

        let new_pid = db
            .create_project(
                "Cyber Studio",
                "D:/work/cyber",
                "Agent research",
                "Be concise.",
                "DEV",
            )
            .expect("create project");
        let tid = db
            .create_task(
                new_pid,
                "Benchmark Q8 KV cache",
                "Compare TTFT",
                TaskPriority::P0,
                "Coder",
            )
            .expect("create task");
        assert!(db.update_task_status(tid, TaskStatus::InProgress));

        let tasks = db.list_tasks(Some(new_pid));
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::InProgress);
        assert_eq!(tasks[0].priority, TaskPriority::P0);

        let cfg = LlamaRuntimeConfig {
            n_gpu_layers: 48,
            kv_cache_type: KvCacheType::Q4_0,
            ..Default::default()
        };
        db.save_llama_config(&cfg);
        db.save_active_project_id(new_pid);
        drop(db);

        let reopened = WorkspaceDb::open_or_memory(tmp.path(), "/workspace");
        assert_eq!(reopened.load_active_project_id(), Some(new_pid));
        let loaded_cfg = reopened.load_llama_config();
        assert_eq!(loaded_cfg.n_gpu_layers, 48);
        assert_eq!(loaded_cfg.kv_cache_type, KvCacheType::Q4_0);
        assert!(loaded_cfg
            .cli_preview(Some("qwen.gguf"))
            .contains("--cache-type-k q4_0"));

        reopened.save_persona(AgentPersona::DeepArchitect);
        let chat_rows = vec![
            PersistedChatRow {
                msg_id: "usr-1".to_string(),
                is_user: true,
                author: "Local Operator".to_string(),
                badge: "OPERATOR".to_string(),
                raw_text: "Analyze workspace latency".to_string(),
                outcome_sha256: None,
                outcome_verified: false,
                citations: vec![],
                receipt_count: 0,
            },
            PersistedChatRow {
                msg_id: "run-1".to_string(),
                is_user: false,
                author: "Lora · qwen".to_string(),
                badge: "LOCAL".to_string(),
                raw_text: "<think>Check KV</think>\nAll nominal.".to_string(),
                outcome_sha256: Some("abc123".to_string()),
                outcome_verified: true,
                citations: vec!["src/main.rs:L1-L10".to_string()],
                receipt_count: 2,
            },
        ];
        reopened.replace_project_chat(new_pid, &chat_rows);
        drop(reopened);

        let third = WorkspaceDb::open_or_memory(tmp.path(), "/workspace");
        assert_eq!(third.load_persona(), AgentPersona::DeepArchitect);
        let reloaded_chat = third.load_project_chat(new_pid);
        assert_eq!(reloaded_chat.len(), 2);
        assert_eq!(reloaded_chat[1].outcome_sha256.as_deref(), Some("abc123"));
        assert!(third.load_project_chat(projects[0].id).is_empty());
    }

    #[test]
    fn sqlite_agent_options_trusted_folders_env_and_daily_done_roundtrip() {
        let tmp = TempDir::new().expect("tempdir");
        let db = WorkspaceDb::open_or_memory(tmp.path(), "D:/Work/Lora");
        let pid = db.list_projects()[0].id;

        db.save_agent_kind(AgentKind::Coder);
        db.save_reasoning_effort(ReasoningEffort::Max);
        db.save_access_mode(ExecutionAccessMode::Api);
        db.save_api_config(&ApiEndpointConfig {
            base_url: "http://127.0.0.1:8080/v1".to_string(),
            model_name: "qwen2.5-coder-32b".to_string(),
            api_key_env: "LORA_API_KEY".to_string(),
        });

        assert_eq!(db.load_agent_kind(), AgentKind::Coder);
        assert_eq!(db.load_reasoning_effort(), ReasoningEffort::Max);
        assert_eq!(db.load_access_mode(), ExecutionAccessMode::Api);
        assert_eq!(db.load_api_config().model_name, "qwen2.5-coder-32b");

        let f_id = db
            .upsert_trusted_folder(pid, "D:/Work/Lora/crates", FolderAccessLevel::Trusted)
            .expect("insert trusted folder");
        assert_eq!(
            db.folder_trust_for_path(pid, "D:\\Work\\Lora\\crates\\lora-core"),
            FolderAccessLevel::Trusted
        );
        assert!(db.update_trusted_folder_level(f_id, FolderAccessLevel::ReadWrite));
        assert_eq!(
            db.folder_trust_for_path(pid, "D:/Work/Lora/crates/lora-core"),
            FolderAccessLevel::ReadWrite
        );

        let imported = db.import_dotenv_text(
            pid,
            "# Comment\nexport LORA_API_KEY=\"sk-demo-123\"\nRUST_LOG=info\n",
        );
        assert_eq!(imported, 2);
        assert_eq!(
            db.resolve_env_value(pid, "LORA_API_KEY").as_deref(),
            Some("sk-demo-123")
        );
        let envs = db.list_env_vars(Some(pid));
        assert!(envs
            .iter()
            .any(|e| e.env_key == "LORA_API_KEY" && e.is_secret));

        let daily_id = db
            .create_task_with_kind(
                pid,
                "Daily code triage",
                "Check build & clippy",
                TaskPriority::P1,
                TaskKind::Daily,
                "Coder",
            )
            .expect("daily task");
        assert!(db.update_task_status(daily_id, TaskStatus::Done));
        let done_task = db
            .list_tasks(Some(pid))
            .into_iter()
            .find(|t| t.id == daily_id)
            .expect("find daily task");
        assert_eq!(done_task.task_kind, TaskKind::Daily);
        assert!(done_task.completed_at_ms.is_some());
    }
}
