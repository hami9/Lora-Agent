//! Scoped document ingestion, SQLite FTS5 lexical retrieval, provenance tracking,
//! and immediate tombstone/forget (`docs/MEMORY_AND_RAG.md`, Phase 6).

use crate::error::{redact_sensitive_text, LoraError};
use crate::policy::{is_excluded_secret_resource, PolicyGate};
use crate::ports::{ScopedFileExecutorPort, ScopedRootHandle};
use crate::store::{now_epoch_ms, SqliteStore};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub chunk_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub doc_sha256: String,
    pub generation: u64,
    pub byte_start: u64,
    pub byte_end: u64,
    pub line_start: u32,
    pub line_end: u32,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievedEvidence {
    pub chunk_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub doc_sha256: String,
    pub byte_range: (u64, u64),
    pub line_range: (u32, u32),
    pub snippet: String,
    pub rank_score: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestionReceipt {
    pub workspace_id: String,
    pub generation: u64,
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    pub skipped_secret_or_invalid: Vec<String>,
}

/// Chunk a UTF-8 document into bounded paragraphs/sections while preserving exact 1-based line ranges
/// and 0-based byte offsets (`docs/MEMORY_AND_RAG.md`).
pub fn chunk_document_text(
    workspace_id: &str,
    relative_path: &str,
    doc_sha256: &str,
    generation: u64,
    text: &str,
    max_chunk_bytes: usize,
) -> Vec<DocumentChunk> {
    let cap = max_chunk_bytes.max(128);
    let mut chunks = Vec::new();
    let mut cur_content = String::new();
    let mut cur_byte_start = 0u64;
    let mut cur_line_start = 1u32;
    let mut byte_cursor = 0u64;

    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (idx, line) in lines.iter().enumerate() {
        let line_no = (idx + 1) as u32;
        if !cur_content.is_empty() && cur_content.len() + line.len() > cap {
            let chunk_id = format!(
                "chk-{}-{}-L{}-L{}",
                &doc_sha256[..10.min(doc_sha256.len())],
                generation,
                cur_line_start,
                line_no - 1
            );
            chunks.push(DocumentChunk {
                chunk_id,
                workspace_id: workspace_id.to_string(),
                relative_path: relative_path.to_string(),
                doc_sha256: doc_sha256.to_string(),
                generation,
                byte_start: cur_byte_start,
                byte_end: byte_cursor,
                line_start: cur_line_start,
                line_end: line_no - 1,
                content: cur_content.trim_end().to_string(),
            });
            cur_content.clear();
            cur_byte_start = byte_cursor;
            cur_line_start = line_no;
        }
        cur_content.push_str(line);
        byte_cursor += line.len() as u64;
    }

    if !cur_content.trim().is_empty() {
        let last_line = lines.len().max(1) as u32;
        let chunk_id = format!(
            "chk-{}-{}-L{}-L{}",
            &doc_sha256[..10.min(doc_sha256.len())],
            generation,
            cur_line_start,
            last_line
        );
        chunks.push(DocumentChunk {
            chunk_id,
            workspace_id: workspace_id.to_string(),
            relative_path: relative_path.to_string(),
            doc_sha256: doc_sha256.to_string(),
            generation,
            byte_start: cur_byte_start,
            byte_end: byte_cursor,
            line_start: cur_line_start,
            line_end: last_line,
            content: cur_content.trim_end().to_string(),
        });
    }

    chunks
}

impl SqliteStore {
    /// Initialize SQLite FTS5 lexical retrieval tables (`docs/MEMORY_AND_RAG.md`).
    pub fn ensure_retrieval_schema(&self) -> Result<(), LoraError> {
        let conn = self.conn_lock();
        conn.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS retrieval_sources (
                workspace_id TEXT PRIMARY KEY,
                active_generation INTEGER NOT NULL,
                tombstoned INTEGER NOT NULL DEFAULT 0,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS retrieval_chunks (
                chunk_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                relative_path TEXT NOT NULL,
                doc_sha256 TEXT NOT NULL,
                byte_start INTEGER NOT NULL,
                byte_end INTEGER NOT NULL,
                line_start INTEGER NOT NULL,
                line_end INTEGER NOT NULL,
                content TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_ws_gen ON retrieval_chunks(workspace_id, generation);
            CREATE VIRTUAL TABLE IF NOT EXISTS retrieval_fts USING fts5(
                chunk_id UNINDEXED,
                workspace_id UNINDEXED,
                relative_path,
                content
            );
            COMMIT;",
        )?;
        Ok(())
    }

    /// Atomically stage a new indexing generation and flip `active_generation` only if the grant
    /// remains valid at commit time (`docs/MEMORY_AND_RAG.md`).
    pub fn index_scoped_files(
        &self,
        root: &ScopedRootHandle,
        relative_paths: &[String],
        max_file_bytes: usize,
        executor: &dyn ScopedFileExecutorPort,
        recheck_grant: &dyn Fn() -> Result<(), LoraError>,
    ) -> Result<IngestionReceipt, LoraError> {
        self.ensure_retrieval_schema()?;
        recheck_grant()?;

        let generation = now_epoch_ms();
        let mut all_chunks = Vec::new();
        let mut skipped = Vec::new();
        let mut indexed_files = 0usize;

        for rel in relative_paths {
            recheck_grant()?;
            if is_excluded_secret_resource(rel) {
                skipped.push(rel.clone());
                continue;
            }
            match executor.read_scoped_file(root, rel, max_file_bytes, recheck_grant) {
                Ok(read_out) => {
                    let doc_chunks = chunk_document_text(
                        &root.root_id,
                        &read_out.relative_path,
                        &read_out.sha256_digest,
                        generation,
                        &read_out.content,
                        512,
                    );
                    all_chunks.extend(doc_chunks);
                    indexed_files += 1;
                }
                Err(_) => {
                    skipped.push(rel.clone());
                }
            }
        }

        // Recheck grant immediately before atomic generation flip (`docs/MEMORY_AND_RAG.md`)
        recheck_grant()?;

        let mut conn = self.conn_lock();
        let tx = conn.transaction()?;

        // Remove previous generations for this workspace before inserting the new generation
        tx.execute(
            "DELETE FROM retrieval_fts WHERE workspace_id = ?1",
            params![root.root_id],
        )?;
        tx.execute(
            "DELETE FROM retrieval_chunks WHERE workspace_id = ?1",
            params![root.root_id],
        )?;

        for chk in &all_chunks {
            let safe_content = redact_sensitive_text(&chk.content);
            tx.execute(
                "INSERT INTO retrieval_chunks (chunk_id, workspace_id, generation, relative_path, doc_sha256, byte_start, byte_end, line_start, line_end, content)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    chk.chunk_id,
                    chk.workspace_id,
                    chk.generation as i64,
                    chk.relative_path,
                    chk.doc_sha256,
                    chk.byte_start as i64,
                    chk.byte_end as i64,
                    chk.line_start,
                    chk.line_end,
                    safe_content
                ],
            )?;
            tx.execute(
                "INSERT INTO retrieval_fts (chunk_id, workspace_id, relative_path, content)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    chk.chunk_id,
                    chk.workspace_id,
                    chk.relative_path,
                    safe_content
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO retrieval_sources (workspace_id, active_generation, tombstoned, updated_at_ms)
             VALUES (?1, ?2, 0, ?3)
             ON CONFLICT(workspace_id) DO UPDATE SET
                 active_generation = excluded.active_generation,
                 tombstoned = 0,
                 updated_at_ms = excluded.updated_at_ms",
            params![root.root_id, generation as i64, now_epoch_ms() as i64],
        )?;

        tx.commit()?;

        Ok(IngestionReceipt {
            workspace_id: root.root_id.clone(),
            generation,
            indexed_files,
            indexed_chunks: all_chunks.len(),
            skipped_secret_or_invalid: skipped,
        })
    }

    /// Query SQLite FTS5 lexical index filtered strictly by `workspace_id`, active non-tombstoned
    /// generation, and live read grant (`docs/MEMORY_AND_RAG.md`).
    pub fn search_lexical_fts(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
        policy: &PolicyGate,
    ) -> Result<Vec<RetrievedEvidence>, LoraError> {
        self.ensure_retrieval_schema()?;

        // Verify workspace root is still registered in policy
        if policy.resolve_root(workspace_id).is_none() {
            return Err(LoraError::GrantRevokedOrExpired);
        }

        let clean_query: String = query
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    ' '
                }
            })
            .collect();
        let terms: Vec<&str> = clean_query.split_whitespace().collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // Quote each sanitized term so natural-language prompts containing FTS5 operators
        // (`AND`, `OR`, `NOT`, `NEAR`) are matched literally instead of raising syntax errors.
        let fts_match_expr = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");

        let conn = self.conn_lock();
        let mut stmt = conn.prepare(
            "SELECT c.chunk_id, c.workspace_id, c.relative_path, c.doc_sha256,
                    c.byte_start, c.byte_end, c.line_start, c.line_end, c.content
             FROM retrieval_fts f
             JOIN retrieval_chunks c ON c.chunk_id = f.chunk_id
             JOIN retrieval_sources s ON s.workspace_id = c.workspace_id
                AND s.active_generation = c.generation
             WHERE f.retrieval_fts MATCH ?1
               AND c.workspace_id = ?2
               AND s.tombstoned = 0
             ORDER BY bm25(retrieval_fts) ASC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(
            params![fts_match_expr, workspace_id, limit.max(1) as i64],
            |row| {
                Ok(RetrievedEvidence {
                    chunk_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    relative_path: row.get(2)?,
                    doc_sha256: row.get(3)?,
                    byte_range: (row.get::<_, i64>(4)? as u64, row.get::<_, i64>(5)? as u64),
                    line_range: (row.get(6)?, row.get(7)?),
                    snippet: row.get(8)?,
                    rank_score: 1,
                })
            },
        )?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }

        let mut digest_hasher = Sha256::new();
        digest_hasher.update(workspace_id.as_bytes());
        let _ = digest_hasher.finalize();

        Ok(results)
    }

    /// Immediately tombstone a workspace scope and purge all derived FTS5 and chunk entries (`docs/MEMORY_AND_RAG.md`).
    pub fn forget_workspace_index(&self, workspace_id: &str) -> Result<usize, LoraError> {
        self.ensure_retrieval_schema()?;
        let mut conn = self.conn_lock();
        let tx = conn.transaction()?;

        tx.execute(
            "UPDATE retrieval_sources SET tombstoned = 1, updated_at_ms = ?1 WHERE workspace_id = ?2",
            params![now_epoch_ms() as i64, workspace_id],
        )?;
        tx.execute(
            "DELETE FROM retrieval_fts WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM retrieval_chunks WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        tx.commit()?;
        Ok(deleted)
    }
}

/// Embedding namespace metadata (`docs/MEMORY_AND_RAG.md`).
/// Prevents mixing vectors across incompatible embedding models, dimensions, or chunker revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingNamespace {
    pub model_digest: String,
    pub tokenizer: String,
    pub dimension: u32,
    pub normalized: bool,
    pub chunker_revision: u32,
    pub parser_revision: u32,
}

impl EmbeddingNamespace {
    /// Verify that a query embedding namespace is strictly compatible with an indexed namespace (`docs/MEMORY_AND_RAG.md`).
    pub fn verify_compatible_namespace(&self, other: &EmbeddingNamespace) -> Result<(), LoraError> {
        if self.dimension == 0 || other.dimension == 0 || self.dimension != other.dimension {
            return Err(LoraError::IncompatibleModel(format!(
                "embedding dimension mismatch: index={} vs query={}",
                self.dimension, other.dimension
            )));
        }
        if self.model_digest != other.model_digest || self.tokenizer != other.tokenizer {
            return Err(LoraError::IncompatibleModel(format!(
                "embedding model/tokenizer namespace mismatch: index=({},{}) vs query=({},{})",
                self.model_digest, self.tokenizer, other.model_digest, other.tokenizer
            )));
        }
        if self.normalized != other.normalized
            || self.chunker_revision != other.chunker_revision
            || self.parser_revision != other.parser_revision
        {
            return Err(LoraError::IncompatibleModel(
                "embedding normalization or chunker/parser revision mismatch".to_string(),
            ));
        }
        Ok(())
    }
}

/// Combine lexical (FTS5) and vector candidate lists using Reciprocal-Rank Fusion (`RRF`)
/// (`docs/MEMORY_AND_RAG.md`). Scores are scaled to integer micro-points (`1_000_000 / (k + rank)`).
pub fn reciprocal_rank_fusion(
    lexical_candidates: &[RetrievedEvidence],
    vector_candidates: &[RetrievedEvidence],
    rrf_k: u32,
    limit: usize,
) -> Vec<RetrievedEvidence> {
    let k = rrf_k.max(1) as i64;
    let mut scores: std::collections::BTreeMap<String, (i64, RetrievedEvidence)> =
        std::collections::BTreeMap::new();

    for (idx, item) in lexical_candidates.iter().enumerate() {
        let rank = (idx + 1) as i64;
        let increment = 1_000_000 / (k + rank);
        let entry = scores
            .entry(item.chunk_id.clone())
            .or_insert_with(|| (0, item.clone()));
        entry.0 += increment;
    }

    for (idx, item) in vector_candidates.iter().enumerate() {
        let rank = (idx + 1) as i64;
        let increment = 1_000_000 / (k + rank);
        let entry = scores
            .entry(item.chunk_id.clone())
            .or_insert_with(|| (0, item.clone()));
        entry.0 += increment;
    }

    let mut fused: Vec<RetrievedEvidence> = scores
        .into_values()
        .map(|(score, mut ev)| {
            ev.rank_score = score;
            ev
        })
        .collect();

    fused.sort_by(|a, b| {
        b.rank_score
            .cmp(&a.rank_score)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    fused.truncate(limit.max(1));
    fused
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudgetRequest {
    pub context_window_tokens: u32,
    pub reserved_output_tokens: u32,
    pub control_schema_overhead_tokens: u32,
    pub user_request_tokens: u32,
    pub recent_turns_tokens: u32,
    pub tool_receipts_tokens: u32,
    /// Ranked retrieved excerpts paired with their estimated token cost.
    pub ranked_excerpts: Vec<(RetrievedEvidence, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudgetAllocation {
    pub context_window_tokens: u32,
    pub reserved_output_tokens: u32,
    pub control_schema_overhead_tokens: u32,
    pub user_request_tokens: u32,
    pub admitted_recent_turns_tokens: u32,
    pub admitted_tool_receipts_tokens: u32,
    pub admitted_excerpts: Vec<RetrievedEvidence>,
    pub dropped_excerpt_ids: Vec<String>,
    pub total_allocated_tokens: u32,
    pub remaining_headroom_tokens: u32,
}

/// Deterministic context window budget allocator (`docs/MEMORY_AND_RAG.md` — "Context budgeting"):
/// 1. Reserves output tokens and control/schema overhead first.
/// 2. Protects the user's current request without silent truncation; fails closed if the user request
///    alone exceeds the remaining input budget.
/// 3. Admits recent turns and tool receipts, then admits highest-ranked retrieved excerpts in order,
///    dropping lower-ranked excerpts when the budget is tight.
pub fn allocate_context_budget(
    req: &ContextBudgetRequest,
) -> Result<ContextBudgetAllocation, LoraError> {
    let mandatory_overhead = req
        .reserved_output_tokens
        .saturating_add(req.control_schema_overhead_tokens);
    let required_floor = mandatory_overhead.saturating_add(req.user_request_tokens);

    if required_floor > req.context_window_tokens {
        return Err(LoraError::BudgetExhausted(format!(
            "user request ({} tokens) + reserved output/schema overhead ({} tokens) exceeds context window ({} tokens)",
            req.user_request_tokens, mandatory_overhead, req.context_window_tokens
        )));
    }

    let mut remaining = req.context_window_tokens - required_floor;

    let admitted_tool_receipts_tokens = req.tool_receipts_tokens.min(remaining);
    remaining -= admitted_tool_receipts_tokens;

    let admitted_recent_turns_tokens = req.recent_turns_tokens.min(remaining);
    remaining -= admitted_recent_turns_tokens;

    let mut admitted_excerpts = Vec::new();
    let mut dropped_excerpt_ids = Vec::new();

    for (ev, token_cost) in &req.ranked_excerpts {
        let cost = (*token_cost).max(1);
        if cost <= remaining {
            remaining -= cost;
            admitted_excerpts.push(ev.clone());
        } else {
            dropped_excerpt_ids.push(ev.chunk_id.clone());
        }
    }

    let total_allocated_tokens = req.context_window_tokens - remaining;

    Ok(ContextBudgetAllocation {
        context_window_tokens: req.context_window_tokens,
        reserved_output_tokens: req.reserved_output_tokens,
        control_schema_overhead_tokens: req.control_schema_overhead_tokens,
        user_request_tokens: req.user_request_tokens,
        admitted_recent_turns_tokens,
        admitted_tool_receipts_tokens,
        admitted_excerpts,
        dropped_excerpt_ids,
        total_allocated_tokens,
        remaining_headroom_tokens: remaining,
    })
}

/// A single persisted or in-memory conversation turn (`docs/MEMORY_AND_RAG.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// Format a multi-turn conversation prompt while strictly enforcing the context window token budget
/// (`docs/MEMORY_AND_RAG.md`). Older turns are dropped first when the budget is tight; the current
/// user request is never silently truncated. Returns `(formatted_prompt, admitted_turn_count)`.
pub fn format_multi_turn_prompt(
    history: &[ChatTurn],
    user_prompt: &str,
    context_window_tokens: u32,
    reserved_output_tokens: u32,
) -> Result<(String, usize), LoraError> {
    let user_request_tokens = ((user_prompt.len() / 4) + 1) as u32;
    let turn_costs: Vec<u32> = history
        .iter()
        .map(|t| ((t.role.len() + t.content.len()) / 4 + 4) as u32)
        .collect();
    let total_history_tokens: u32 = turn_costs.iter().copied().sum();

    let alloc = allocate_context_budget(&ContextBudgetRequest {
        context_window_tokens,
        reserved_output_tokens,
        control_schema_overhead_tokens: 32,
        user_request_tokens,
        recent_turns_tokens: total_history_tokens,
        tool_receipts_tokens: 0,
        ranked_excerpts: Vec::new(),
    })?;

    if history.is_empty() || alloc.admitted_recent_turns_tokens == 0 {
        return Ok((user_prompt.to_string(), 0));
    }

    let mut remaining_turn_budget = alloc.admitted_recent_turns_tokens;
    let mut selected_indices = Vec::new();
    for (idx, &cost) in turn_costs.iter().enumerate().rev() {
        let cost = cost.max(1);
        if cost <= remaining_turn_budget {
            remaining_turn_budget -= cost;
            selected_indices.push(idx);
        } else {
            break;
        }
    }
    selected_indices.reverse();

    if selected_indices.is_empty() {
        return Ok((user_prompt.to_string(), 0));
    }

    let mut out = String::new();
    for &idx in &selected_indices {
        let turn = &history[idx];
        let tag = turn.role.trim().to_ascii_uppercase();
        out.push_str(&format!("[{tag}]: {}\n", turn.content.trim()));
    }
    out.push_str(&format!("[USER]: {user_prompt}"));
    Ok((out, selected_indices.len()))
}
