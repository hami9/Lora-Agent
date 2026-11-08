//! Single-writer SQLite persistence, schema migrations, backups, audit journal,
//! and startup crash reconciliation (`docs/ARCHITECTURE.md`, `ADR-0005`, `M7`).

use crate::agent::WorkflowExecutionReceipt;
use crate::error::{redact_sensitive_text, LoraError};
use rusqlite::{backup::Backup, params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

pub type StoredModelRow = (String, String, String, u64, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycleState {
    Staging,
    Verifying,
    Probing,
    Ready,
    Leased,
    Retired,
    Rejected,
    Collected,
}

impl ModelLifecycleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Verifying => "verifying",
            Self::Probing => "probing",
            Self::Ready => "ready",
            Self::Leased => "leased",
            Self::Retired => "retired",
            Self::Rejected => "rejected",
            Self::Collected => "collected",
        }
    }

    pub fn parse_state(s: &str) -> Self {
        match s {
            "staging" => Self::Staging,
            "verifying" => Self::Verifying,
            "probing" => Self::Probing,
            "ready" => Self::Ready,
            "leased" => Self::Leased,
            "retired" => Self::Retired,
            "rejected" => Self::Rejected,
            _ => Self::Collected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Planning,
    Checking,
    AwaitingApproval,
    Executing,
    Observing,
    Verifying,
    Succeeded,
    Failed,
    Interrupted,
    Cancelled,
}

impl RunState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Checking => "checking",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Executing => "executing",
            Self::Observing => "observing",
            Self::Verifying => "verifying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse_state(s: &str) -> Self {
        match s {
            "planning" => Self::Planning,
            "checking" => Self::Checking,
            "awaiting_approval" => Self::AwaitingApproval,
            "executing" => Self::Executing,
            "observing" => Self::Observing,
            "verifying" => Self::Verifying,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Interrupted,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Interrupted | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: i64,
    pub run_id: String,
    pub actor: String,
    pub policy_revision: u32,
    pub action_digest: String,
    pub resource_alias: String,
    pub decision: String,
    pub outcome: String,
    pub summary: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationReport {
    pub interrupted_runs: Vec<String>,
    pub interrupted_invocations: Vec<String>,
    pub cleaned_staging_files: usize,
    pub rejected_partial_imports: Vec<String>,
}

/// Single-writer SQLite store managing model revisions, run state transitions, tool receipts, and redacted audit events.
#[derive(Clone)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    simulate_journal_failure: Arc<AtomicBool>,
}

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl SqliteStore {
    /// Open or initialize the SQLite database at `db_path`, checking schema version compatibility
    /// and taking a coherent backup before migrating (`ADR-0005`).
    pub fn open(db_path: &Path) -> Result<Self, LoraError> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.busy_timeout(Duration::from_millis(2_500))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let user_version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

        if user_version > CURRENT_SCHEMA_VERSION {
            return Err(LoraError::SchemaVersionTooNew {
                found: user_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        if user_version < CURRENT_SCHEMA_VERSION {
            if user_version > 0 && db_path.exists() {
                let backup_path = db_path.with_extension(format!("v{user_version}.bak"));
                let mut dst = Connection::open(&backup_path)?;
                let backup = Backup::new(&conn, &mut dst)?;
                backup.run_to_completion(100, Duration::from_millis(10), None)?;
            }
            Self::apply_migrations(&conn)?;
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
            simulate_journal_failure: Arc::new(AtomicBool::new(false)),
        })
    }

    fn apply_migrations(conn: &Connection) -> Result<(), LoraError> {
        conn.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS model_revisions (
                revision_id TEXT PRIMARY KEY,
                display_alias TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                state TEXT NOT NULL,
                manifest_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_model_sha256 ON model_revisions(sha256);
            CREATE INDEX IF NOT EXISTS idx_model_alias ON model_revisions(display_alias);

            CREATE TABLE IF NOT EXISTS runs (
                run_id TEXT PRIMARY KEY,
                principal TEXT NOT NULL,
                workspace TEXT NOT NULL,
                mode TEXT NOT NULL,
                model_digest TEXT NOT NULL,
                state TEXT NOT NULL,
                state_revision INTEGER NOT NULL,
                terminal_reason TEXT,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tool_invocations (
                invocation_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                turn INTEGER NOT NULL,
                tool_id TEXT NOT NULL,
                action_digest TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                status TEXT NOT NULL,
                receipt_json TEXT,
                created_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS audit_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                actor TEXT NOT NULL,
                policy_revision INTEGER NOT NULL,
                action_digest TEXT NOT NULL,
                resource_alias TEXT NOT NULL,
                decision TEXT NOT NULL,
                outcome TEXT NOT NULL,
                summary TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS workflow_receipts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workflow_id TEXT NOT NULL,
                total_steps INTEGER NOT NULL,
                completed_steps INTEGER NOT NULL,
                successful INTEGER NOT NULL,
                receipt_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_workflow_receipts_wid ON workflow_receipts(workflow_id);

            PRAGMA user_version = 1;
            COMMIT;",
        )?;
        Ok(())
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub(crate) fn conn_lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store mutex poisoned")
    }

    /// Enable or disable fault-injected audit/journal failure for testing `M7` & `docs/SECURITY_ARCHITECTURE.md`.
    pub fn set_simulate_journal_failure(&self, fail: bool) {
        self.simulate_journal_failure.store(fail, Ordering::SeqCst);
    }

    /// Reconcile unfinished runs, in-flight tool intents, partial imports, and staging files on startup (`M7`).
    /// Never replays a tool invocation after an uncertain crash.
    pub fn reconcile_on_startup(
        &self,
        staging_dir: &Path,
        blobs_dir: &Path,
    ) -> Result<ReconciliationReport, LoraError> {
        let mut interrupted_runs = Vec::new();
        let mut interrupted_invocations = Vec::new();
        let mut rejected_partial_imports = Vec::new();
        let now = now_epoch_ms();

        {
            let mut conn = self.conn.lock().expect("store mutex poisoned");
            let tx = conn.transaction()?;

            {
                let mut stmt = tx.prepare(
                    "SELECT run_id FROM runs WHERE state NOT IN ('succeeded', 'failed', 'interrupted', 'cancelled')",
                )?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for r in rows {
                    interrupted_runs.push(r?);
                }
            }

            for run_id in &interrupted_runs {
                tx.execute(
                    "UPDATE runs SET state = 'interrupted', state_revision = state_revision + 1, terminal_reason = 'interrupted_by_restart', updated_at_ms = ?1 WHERE run_id = ?2",
                    params![now, run_id],
                )?;
                tx.execute(
                    "INSERT INTO audit_events (run_id, actor, policy_revision, action_digest, resource_alias, decision, outcome, summary, created_at_ms)
                     VALUES (?1, 'system', 1, 'startup_reconcile', 'run', 'reconcile', 'interrupted', 'Marked unfinished run interrupted on startup without replaying tools', ?2)",
                    params![run_id, now],
                )?;
            }

            {
                let mut stmt = tx.prepare(
                    "SELECT invocation_id FROM tool_invocations WHERE status IN ('intent_recorded', 'executing')",
                )?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for inv in rows {
                    interrupted_invocations.push(inv?);
                }
            }

            for inv_id in &interrupted_invocations {
                tx.execute(
                    "UPDATE tool_invocations SET status = 'interrupted' WHERE invocation_id = ?1",
                    params![inv_id],
                )?;
            }

            {
                let mut stmt = tx.prepare(
                    "SELECT revision_id, sha256, state FROM model_revisions WHERE state IN ('staging', 'verifying', 'probing', 'ready', 'leased')",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                for row in rows {
                    let (rev_id, sha256, state) = row?;
                    if matches!(state.as_str(), "staging" | "verifying" | "probing") {
                        rejected_partial_imports.push(rev_id.clone());
                        tx.execute(
                            "UPDATE model_revisions SET state = 'rejected' WHERE revision_id = ?1",
                            params![rev_id],
                        )?;
                    } else if state == "leased" {
                        // Release stale worker lease after restart
                        tx.execute(
                            "UPDATE model_revisions SET state = 'ready' WHERE revision_id = ?1",
                            params![rev_id],
                        )?;
                    } else if state == "ready" {
                        // Verify backing content-addressed blob still exists on disk
                        let blob_path = blobs_dir.join(format!("{sha256}.gguf"));
                        if !blob_path.is_file() {
                            rejected_partial_imports.push(rev_id.clone());
                            tx.execute(
                                "UPDATE model_revisions SET state = 'rejected' WHERE revision_id = ?1",
                                params![rev_id],
                            )?;
                        }
                    }
                }
            }

            tx.commit()?;
        }

        let mut cleaned_staging_files = 0usize;
        if staging_dir.is_dir() {
            for entry in fs::read_dir(staging_dir)? {
                let entry = entry?;
                if entry.path().is_file() {
                    let _ = fs::remove_file(entry.path());
                    cleaned_staging_files += 1;
                }
            }
        }

        Ok(ReconciliationReport {
            interrupted_runs,
            interrupted_invocations,
            cleaned_staging_files,
            rejected_partial_imports,
        })
    }

    pub fn insert_model_staging(
        &self,
        revision_id: &str,
        display_alias: &str,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<(), LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO model_revisions (revision_id, display_alias, sha256, size_bytes, state, manifest_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, 'staging', '{}', ?5)",
            params![revision_id, display_alias, sha256, size_bytes as i64, now_epoch_ms() as i64],
        )?;
        Ok(())
    }

    pub fn update_model_state(
        &self,
        revision_id: &str,
        state: ModelLifecycleState,
        manifest_json: Option<&str>,
    ) -> Result<(), LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        if let Some(manifest) = manifest_json {
            conn.execute(
                "UPDATE model_revisions SET state = ?1, manifest_json = ?2 WHERE revision_id = ?3",
                params![state.as_str(), manifest, revision_id],
            )?;
        } else {
            conn.execute(
                "UPDATE model_revisions SET state = ?1 WHERE revision_id = ?2",
                params![state.as_str(), revision_id],
            )?;
        }
        Ok(())
    }

    pub fn find_ready_model_by_sha256(
        &self,
        sha256: &str,
    ) -> Result<Option<(String, String, String)>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT revision_id, display_alias, manifest_json FROM model_revisions WHERE sha256 = ?1 AND state IN ('ready', 'leased') LIMIT 1",
            params![sha256],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(LoraError::from)
    }

    pub fn find_ready_model_by_id_or_alias(
        &self,
        id_or_alias: &str,
    ) -> Result<Option<StoredModelRow>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT revision_id, display_alias, sha256, size_bytes, manifest_json
             FROM model_revisions
             WHERE (revision_id = ?1 OR display_alias = ?1 OR sha256 = ?1)
               AND state IN ('ready', 'leased')
             ORDER BY created_at_ms DESC LIMIT 1",
            params![id_or_alias],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? as u64,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(LoraError::from)
    }

    pub fn list_ready_models(&self) -> Result<Vec<StoredModelRow>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT revision_id, display_alias, sha256, size_bytes, manifest_json
             FROM model_revisions
             WHERE state IN ('ready', 'leased')
             ORDER BY created_at_ms DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3)? as u64,
                row.get(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn create_run(
        &self,
        run_id: &str,
        principal: &str,
        workspace: &str,
        mode: &str,
        model_digest: &str,
    ) -> Result<u32, LoraError> {
        if self.simulate_journal_failure.load(Ordering::SeqCst) {
            return Err(LoraError::AuditFailure(
                "simulated disk-full or journal write error".to_string(),
            ));
        }
        let now = now_epoch_ms() as i64;
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO runs (run_id, principal, workspace, mode, model_digest, state, state_revision, terminal_reason, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'planning', 1, NULL, ?6, ?6)",
            params![run_id, principal, workspace, mode, model_digest, now],
        )?;
        Ok(1)
    }

    /// Compare-and-swap a run state transition (`docs/AGENT_RUNTIME.md`).
    /// Guarantees that at most one terminal transition wins completion vs cancellation.
    pub fn transition_run_state(
        &self,
        run_id: &str,
        expected_revision: u32,
        next_state: RunState,
        terminal_reason: Option<&str>,
    ) -> Result<Option<u32>, LoraError> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        let current: Option<(String, u32)> = tx
            .query_row(
                "SELECT state, state_revision FROM runs WHERE run_id = ?1",
                params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((cur_state_str, cur_rev)) = current else {
            return Ok(None);
        };
        let cur_state = RunState::parse_state(&cur_state_str);
        if cur_state.is_terminal() || cur_rev != expected_revision {
            return Ok(None);
        }

        let new_rev = cur_rev + 1;
        let sanitized_reason = terminal_reason.map(redact_sensitive_text);
        tx.execute(
            "UPDATE runs SET state = ?1, state_revision = ?2, terminal_reason = ?3, updated_at_ms = ?4 WHERE run_id = ?5 AND state_revision = ?6",
            params![
                next_state.as_str(),
                new_rev,
                sanitized_reason,
                now_epoch_ms() as i64,
                run_id,
                expected_revision
            ],
        )?;
        tx.commit()?;
        Ok(Some(new_rev))
    }

    /// Force transition to a terminal state (e.g., on cancellation race or worker crash) only if the run is still non-terminal.
    /// Returns `true` if this call won the single terminal transition.
    pub fn finalize_run_if_nonterminal(
        &self,
        run_id: &str,
        terminal_state: RunState,
        reason: &str,
    ) -> Result<bool, LoraError> {
        debug_assert!(terminal_state.is_terminal());
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        let current: Option<(String, u32)> = tx
            .query_row(
                "SELECT state, state_revision FROM runs WHERE run_id = ?1",
                params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((cur_state_str, cur_rev)) = current else {
            return Ok(false);
        };
        if RunState::parse_state(&cur_state_str).is_terminal() {
            return Ok(false);
        }
        let new_rev = cur_rev + 1;
        let sanitized = redact_sensitive_text(reason);
        let updated = tx.execute(
            "UPDATE runs SET state = ?1, state_revision = ?2, terminal_reason = ?3, updated_at_ms = ?4 WHERE run_id = ?5 AND state_revision = ?6",
            params![
                terminal_state.as_str(),
                new_rev,
                sanitized,
                now_epoch_ms() as i64,
                run_id,
                cur_rev
            ],
        )?;
        tx.commit()?;
        Ok(updated == 1)
    }

    pub fn get_run_state(
        &self,
        run_id: &str,
    ) -> Result<Option<(RunState, u32, Option<String>)>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT state, state_revision, terminal_reason FROM runs WHERE run_id = ?1",
            params![run_id],
            |row| {
                let s: String = row.get(0)?;
                let rev: u32 = row.get(1)?;
                let reason: Option<String> = row.get(2)?;
                Ok((RunState::parse_state(&s), rev, reason))
            },
        )
        .optional()
        .map_err(LoraError::from)
    }

    /// Persist tool invocation intent before dispatch (`docs/AGENT_RUNTIME.md`).
    /// Fails with `LoraError::AuditFailure` if journal writes are unavailable, blocking tool dispatch.
    pub fn record_tool_intent(
        &self,
        invocation_id: &str,
        run_id: &str,
        turn: u32,
        tool_id: &str,
        action_digest: &str,
        relative_path: &str,
    ) -> Result<(), LoraError> {
        if self.simulate_journal_failure.load(Ordering::SeqCst) {
            return Err(LoraError::AuditFailure(
                "cannot persist tool invocation intent before dispatch (journal unavailable)"
                    .to_string(),
            ));
        }
        let conn = self.conn.lock().expect("store mutex poisoned");
        let existing: Option<String> = conn
            .query_row(
                "SELECT status FROM tool_invocations WHERE invocation_id = ?1",
                params![invocation_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(LoraError::PolicyDenied(format!(
                "duplicate tool invocation ID '{invocation_id}' cannot be re-dispatched"
            )));
        }
        conn.execute(
            "INSERT INTO tool_invocations (invocation_id, run_id, turn, tool_id, action_digest, relative_path, status, receipt_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'intent_recorded', NULL, ?7)",
            params![
                invocation_id,
                run_id,
                turn,
                tool_id,
                action_digest,
                redact_sensitive_text(relative_path),
                now_epoch_ms() as i64
            ],
        )?;
        Ok(())
    }

    pub fn complete_tool_invocation(
        &self,
        invocation_id: &str,
        status: &str,
        receipt_json: &str,
    ) -> Result<(), LoraError> {
        if self.simulate_journal_failure.load(Ordering::SeqCst) {
            return Err(LoraError::AuditFailure(
                "cannot commit tool receipt to journal".to_string(),
            ));
        }
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "UPDATE tool_invocations SET status = ?1, receipt_json = ?2 WHERE invocation_id = ?3",
            params![status, redact_sensitive_text(receipt_json), invocation_id],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_audit(
        &self,
        run_id: &str,
        actor: &str,
        policy_revision: u32,
        action_digest: &str,
        resource_alias: &str,
        decision: &str,
        outcome: &str,
        summary: &str,
    ) -> Result<i64, LoraError> {
        if self.simulate_journal_failure.load(Ordering::SeqCst) {
            return Err(LoraError::AuditFailure(
                "audit journal write failed".to_string(),
            ));
        }
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO audit_events (run_id, actor, policy_revision, action_digest, resource_alias, decision, outcome, summary, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run_id,
                actor,
                policy_revision,
                action_digest,
                redact_sensitive_text(resource_alias),
                decision,
                outcome,
                redact_sensitive_text(summary),
                now_epoch_ms() as i64
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_audit_events(
        &self,
        run_id_filter: Option<&str>,
    ) -> Result<Vec<AuditRecord>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut records = Vec::new();
        let sql = if run_id_filter.is_some() {
            "SELECT seq, run_id, actor, policy_revision, action_digest, resource_alias, decision, outcome, summary, created_at_ms
             FROM audit_events WHERE run_id = ?1 ORDER BY seq ASC"
        } else {
            "SELECT seq, run_id, actor, policy_revision, action_digest, resource_alias, decision, outcome, summary, created_at_ms
             FROM audit_events ORDER BY seq ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(AuditRecord {
                seq: row.get(0)?,
                run_id: row.get(1)?,
                actor: row.get(2)?,
                policy_revision: row.get(3)?,
                action_digest: row.get(4)?,
                resource_alias: row.get(5)?,
                decision: row.get(6)?,
                outcome: row.get(7)?,
                summary: row.get(8)?,
                created_at_ms: row.get::<_, i64>(9)? as u64,
            })
        };
        if let Some(rid) = run_id_filter {
            for r in stmt.query_map(params![rid], map_row)? {
                records.push(r?);
            }
        } else {
            for r in stmt.query_map([], map_row)? {
                records.push(r?);
            }
        }
        Ok(records)
    }

    /// Prune older audit records while retaining at most `keep_latest` most recent entries (`docs/SECURITY_ARCHITECTURE.md`, `R17`).
    pub fn prune_audit_retention(&self, keep_latest: usize) -> Result<usize, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let deleted = conn.execute(
            "DELETE FROM audit_events WHERE seq NOT IN (
                SELECT seq FROM audit_events ORDER BY seq DESC LIMIT ?1
            )",
            params![keep_latest as i64],
        )?;
        Ok(deleted)
    }

    /// Export a redacted JSON diagnostic bundle of audit records (`docs/SECURITY_ARCHITECTURE.md`, `R17`).
    pub fn export_redacted_audit_bundle(
        &self,
        run_id_filter: Option<&str>,
    ) -> Result<String, LoraError> {
        let events = self.list_audit_events(run_id_filter)?;
        let raw_json = serde_json::to_string_pretty(&events)
            .map_err(|e| LoraError::Database(e.to_string()))?;
        Ok(redact_sensitive_text(&raw_json))
    }

    /// Persist a workflow execution receipt (`M7`, `docs/ARCHITECTURE.md`).
    pub fn record_workflow_receipt(
        &self,
        receipt: &WorkflowExecutionReceipt,
    ) -> Result<i64, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let receipt_json =
            serde_json::to_string(receipt).map_err(|e| LoraError::Database(e.to_string()))?;
        conn.execute(
            "INSERT INTO workflow_receipts (workflow_id, total_steps, completed_steps, successful, receipt_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                receipt.workflow_id,
                receipt.total_steps as i64,
                receipt.completed_steps as i64,
                if receipt.successful { 1i64 } else { 0i64 },
                receipt_json,
                receipt.created_at_ms as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// List recent workflow execution receipts ordered by recency (`M7`, `docs/ARCHITECTURE.md`).
    pub fn list_workflow_receipts(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<WorkflowExecutionReceipt>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let lim = limit.unwrap_or(50) as i64;
        let mut stmt =
            conn.prepare("SELECT receipt_json FROM workflow_receipts ORDER BY id DESC LIMIT ?1")?;
        let mut receipts = Vec::new();
        let rows = stmt.query_map(params![lim], |row| row.get::<_, String>(0))?;
        for r in rows {
            let json_str = r?;
            if let Ok(rcpt) = serde_json::from_str::<WorkflowExecutionReceipt>(&json_str) {
                receipts.push(rcpt);
            }
        }
        Ok(receipts)
    }

    /// Find the most recent workflow execution receipt by workflow ID (`M7`, `docs/ARCHITECTURE.md`).
    pub fn find_workflow_receipt(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowExecutionReceipt>, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT receipt_json FROM workflow_receipts WHERE workflow_id = ?1 ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![workflow_id], |row| row.get::<_, String>(0))?;
        if let Some(r) = rows.next() {
            let json_str = r?;
            let rcpt = serde_json::from_str::<WorkflowExecutionReceipt>(&json_str)
                .map_err(|e| LoraError::Database(e.to_string()))?;
            Ok(Some(rcpt))
        } else {
            Ok(None)
        }
    }

    /// Prune older workflow receipts while retaining at most `keep_latest` entries.
    pub fn prune_workflow_receipts(&self, keep_latest: usize) -> Result<usize, LoraError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let deleted = conn.execute(
            "DELETE FROM workflow_receipts WHERE id NOT IN (
                SELECT id FROM workflow_receipts ORDER BY id DESC LIMIT ?1
            )",
            params![keep_latest as i64],
        )?;
        Ok(deleted)
    }

    /// Audit cryptographic attestation integrity across recent persisted workflow receipts.
    /// Returns `(total_attested_checked, verified_ok_count)`.
    pub fn verify_stored_workflow_receipts(
        &self,
        limit: Option<usize>,
    ) -> Result<(usize, usize), LoraError> {
        let receipts = self.list_workflow_receipts(limit)?;
        let mut checked = 0usize;
        let mut valid = 0usize;
        for rcpt in &receipts {
            if !rcpt.receipt_sha256.is_empty() {
                checked += 1;
                if crate::agent::verify_receipt_self_integrity(rcpt).is_ok() {
                    valid += 1;
                }
            }
        }
        Ok((checked, valid))
    }

    /// Export a cryptographically bound `WorkflowAttestationBundle` over recent workflow receipts.
    pub fn export_workflow_attestation_bundle(
        &self,
        limit: Option<usize>,
    ) -> Result<crate::agent::WorkflowAttestationBundle, LoraError> {
        let receipts = self.list_workflow_receipts(limit)?;
        Ok(crate::agent::build_workflow_attestation_bundle(receipts))
    }
}

/// Compute a keyed correlation digest over a sensitive short string using a per-install salt (`docs/SECURITY_ARCHITECTURE.md`).
pub fn keyed_correlation_digest(install_key: &str, sensitive_short_value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"lora-correlation-v1:");
    hasher.update(install_key.as_bytes());
    hasher.update(b":");
    hasher.update(sensitive_short_value.as_bytes());
    hex::encode(hasher.finalize())
}
