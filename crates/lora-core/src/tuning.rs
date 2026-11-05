//! Phase 7 Hardware Intelligence auto-tuning receipts and bounded calibration grid
//! (`docs/HARDWARE_INTELLIGENCE.md`, `R16`).

use crate::admission::TuningProfile;
use crate::error::LoraError;
use crate::store::{now_epoch_ms, SqliteStore};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Composite key identifying a specific hardware, OS, worker build, model, context, and profile tuple (`docs/HARDWARE_INTELLIGENCE.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuningCacheKey {
    pub hardware_fingerprint: String,
    pub os_arch: String,
    pub worker_revision: String,
    pub model_sha256: String,
    pub context_length: u32,
    pub profile: TuningProfile,
}

impl TuningCacheKey {
    pub fn key_digest(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        hex::encode(Sha256::digest(json.as_bytes()))
    }
}

/// Measured tuning receipt persisted in SQLite (`docs/HARDWARE_INTELLIGENCE.md`).
/// Automatically invalidated when `worker_revision`, `hardware_fingerprint`, or `model_sha256` changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuningReceipt {
    pub key_digest: String,
    pub key: TuningCacheKey,
    pub optimal_threads: u32,
    pub optimal_batch_size: u32,
    pub baseline_tok_per_sec_x100: u32,
    pub tuned_tok_per_sec_x100: u32,
    pub variance_pct: u32,
    pub peak_rss_bytes: u64,
    pub aborted_due_to_pressure_or_thermal: bool,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuningTrialSample {
    pub threads: u32,
    pub batch_size: u32,
    pub tok_per_sec_x100: u32,
    pub variance_pct: u32,
    pub peak_rss_bytes: u64,
    pub thermal_or_memory_warning: bool,
}

impl SqliteStore {
    pub fn ensure_tuning_schema(&self) -> Result<(), LoraError> {
        let conn = self.conn_lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tuning_receipts (
                key_digest TEXT PRIMARY KEY,
                worker_revision TEXT NOT NULL,
                model_sha256 TEXT NOT NULL,
                receipt_json TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Run a bounded tuning grid evaluation (`docs/HARDWARE_INTELLIGENCE.md`, `R16`):
    /// - Immediately aborts if any trial triggers thermal or memory pressure (`thermal_or_memory_warning`).
    /// - Only promotes a tuned configuration over static baseline if throughput improves by >= 5% with variance <= 10%.
    pub fn evaluate_and_store_tuning_receipt(
        &self,
        key: TuningCacheKey,
        baseline_threads: u32,
        baseline_batch: u32,
        baseline_tps_x100: u32,
        trials: &[TuningTrialSample],
        ttl_ms: u64,
    ) -> Result<TuningReceipt, LoraError> {
        self.ensure_tuning_schema()?;
        let now = now_epoch_ms();
        let key_digest = key.key_digest();

        // Check if any trial triggered thermal throttling or memory pressure
        if trials.iter().any(|t| t.thermal_or_memory_warning) {
            let fallback = TuningReceipt {
                key_digest: key_digest.clone(),
                key,
                optimal_threads: baseline_threads,
                optimal_batch_size: baseline_batch,
                baseline_tok_per_sec_x100: baseline_tps_x100,
                tuned_tok_per_sec_x100: baseline_tps_x100,
                variance_pct: 0,
                peak_rss_bytes: 0,
                aborted_due_to_pressure_or_thermal: true,
                created_at_ms: now,
                expires_at_ms: now.saturating_add(ttl_ms),
            };
            return Ok(fallback);
        }

        // Select best trial with variance <= 10% and at least 5% gain over static baseline (`R16`)
        let min_required_tps = baseline_tps_x100.saturating_mul(105) / 100;
        let best = trials
            .iter()
            .filter(|t| t.variance_pct <= 10 && t.tok_per_sec_x100 >= min_required_tps)
            .max_by_key(|t| t.tok_per_sec_x100);

        let receipt = match best {
            Some(winner) => TuningReceipt {
                key_digest: key_digest.clone(),
                key: key.clone(),
                optimal_threads: winner.threads,
                optimal_batch_size: winner.batch_size,
                baseline_tok_per_sec_x100: baseline_tps_x100,
                tuned_tok_per_sec_x100: winner.tok_per_sec_x100,
                variance_pct: winner.variance_pct,
                peak_rss_bytes: winner.peak_rss_bytes,
                aborted_due_to_pressure_or_thermal: false,
                created_at_ms: now,
                expires_at_ms: now.saturating_add(ttl_ms),
            },
            None => TuningReceipt {
                key_digest: key_digest.clone(),
                key: key.clone(),
                optimal_threads: baseline_threads,
                optimal_batch_size: baseline_batch,
                baseline_tok_per_sec_x100: baseline_tps_x100,
                tuned_tok_per_sec_x100: baseline_tps_x100,
                variance_pct: 0,
                peak_rss_bytes: 0,
                aborted_due_to_pressure_or_thermal: false,
                created_at_ms: now,
                expires_at_ms: now.saturating_add(ttl_ms),
            },
        };

        let json =
            serde_json::to_string(&receipt).map_err(|e| LoraError::Database(e.to_string()))?;
        let conn = self.conn_lock();
        conn.execute(
            "INSERT INTO tuning_receipts (key_digest, worker_revision, model_sha256, receipt_json, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key_digest) DO UPDATE SET
                 worker_revision = excluded.worker_revision,
                 receipt_json = excluded.receipt_json,
                 expires_at_ms = excluded.expires_at_ms",
            params![
                key_digest,
                key.worker_revision,
                key.model_sha256,
                json,
                receipt.expires_at_ms as i64
            ],
        )?;

        Ok(receipt)
    }

    /// Lookup a valid non-expired tuning receipt matching the exact `TuningCacheKey`.
    pub fn get_valid_tuning_receipt(
        &self,
        key: &TuningCacheKey,
        now_ms: u64,
    ) -> Result<Option<TuningReceipt>, LoraError> {
        self.ensure_tuning_schema()?;
        let key_digest = key.key_digest();
        let conn = self.conn_lock();
        let row: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT worker_revision, receipt_json, expires_at_ms FROM tuning_receipts WHERE key_digest = ?1",
                params![key_digest],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((stored_rev, json, exp)) = row else {
            return Ok(None);
        };
        if stored_rev != key.worker_revision || (now_ms as i64) > exp {
            return Ok(None);
        }
        let parsed: TuningReceipt =
            serde_json::from_str(&json).map_err(|e| LoraError::Database(e.to_string()))?;
        Ok(Some(parsed))
    }

    /// List all persisted tuning receipts.
    pub fn list_tuning_receipts(&self) -> Result<Vec<TuningReceipt>, LoraError> {
        self.ensure_tuning_schema()?;
        let conn = self.conn_lock();
        let mut stmt =
            conn.prepare("SELECT receipt_json FROM tuning_receipts ORDER BY expires_at_ms DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut receipts = Vec::new();
        for r in rows {
            let json = r?;
            if let Ok(rec) = serde_json::from_str::<TuningReceipt>(&json) {
                receipts.push(rec);
            }
        }
        Ok(receipts)
    }
}
