//! Redacted structured errors and reason codes (`docs/ARCHITECTURE.md`, `docs/SECURITY_ARCHITECTURE.md`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Machine-readable reason codes shared between CLI and future desktop/API surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    StoreBusy,
    SchemaVersionTooNew,
    DiskFull,
    DigestMismatch,
    MalformedArtifact,
    IncompatibleModel,
    InsufficientMemory,
    PressureChangedAfterReservation,
    ActiveGenerationBusy,
    WorkerHandshakeFailed,
    WorkerIncompatibleVersion,
    WorkerCorruptFrame,
    WorkerTimeout,
    WorkerCrashed,
    Cancelled,
    PolicyDenied,
    ScopeEscapeAttempt,
    GrantRevokedOrExpired,
    ApprovalDeclined,
    InsufficientEvidence,
    BudgetExhausted,
    AuditFailure,
    IoError,
    DatabaseError,
}

#[derive(Debug, Error)]
pub enum LoraError {
    #[error("store is busy (owned by another active Lora host process); retry after the active command completes: {0}")]
    StoreBusy(String),

    #[error("database schema version {found} is newer than this binary supports ({supported}); refusing writes")]
    SchemaVersionTooNew { found: u32, supported: u32 },

    #[error("insufficient disk space for safe staging/commit (required {required_bytes} B, available {available_bytes} B)")]
    DiskFull {
        required_bytes: u64,
        available_bytes: u64,
    },

    #[error("artifact SHA-256 digest mismatch (expected {expected}, computed {actual})")]
    DigestMismatch { expected: String, actual: String },

    #[error("malformed or unsupported model artifact: {0}")]
    MalformedArtifact(String),

    #[error("incompatible model or capability: {0}")]
    IncompatibleModel(String),

    #[error("admission refused due to insufficient memory: {explanation}")]
    InsufficientMemory {
        explanation: String,
        estimated_weights_bytes: u64,
        estimated_kv_bytes: u64,
        available_headroom_bytes: u64,
        suggested_context_length: Option<u32>,
    },

    #[error("memory pressure changed after reservation before model load ({details}); aborting load safely")]
    PressureChangedAfterReservation { details: String },

    #[error(
        "admission refused: another generation is already active in this session; retry when idle"
    )]
    ActiveGenerationBusy,

    #[error("worker handshake failed: {0}")]
    WorkerHandshakeFailed(String),

    #[error("worker protocol major version mismatch: {0}")]
    WorkerIncompatibleVersion(String),

    #[error("worker emitted corrupt or invalid protocol frame: {0}")]
    WorkerCorruptFrame(String),

    #[error("worker exceeded deadline and was terminated: {0}")]
    WorkerTimeout(String),

    #[error("worker process exited unexpectedly during operation: {0}")]
    WorkerCrashed(String),

    #[error("operation cancelled by user or deadline")]
    Cancelled,

    #[error("policy denied tool invocation: {0}")]
    PolicyDenied(String),

    #[error("scoped file access blocked path or link escape attempt: {0}")]
    ScopeEscapeAttempt(String),

    #[error("grant was revoked or expired before completion")]
    GrantRevokedOrExpired,

    #[error("user declined tool approval request")]
    ApprovalDeclined,

    #[error("insufficient evidence in approved file scope to answer the question: {0}")]
    InsufficientEvidence(String),

    #[error("agent run budget exhausted: {0}")]
    BudgetExhausted(String),

    #[error("audit journal failure (refusing new tool effects): {0}")]
    AuditFailure(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("database error: {0}")]
    Database(String),
}

impl LoraError {
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::StoreBusy(_) => ReasonCode::StoreBusy,
            Self::SchemaVersionTooNew { .. } => ReasonCode::SchemaVersionTooNew,
            Self::DiskFull { .. } => ReasonCode::DiskFull,
            Self::DigestMismatch { .. } => ReasonCode::DigestMismatch,
            Self::MalformedArtifact(_) => ReasonCode::MalformedArtifact,
            Self::IncompatibleModel(_) => ReasonCode::IncompatibleModel,
            Self::InsufficientMemory { .. } => ReasonCode::InsufficientMemory,
            Self::PressureChangedAfterReservation { .. } => {
                ReasonCode::PressureChangedAfterReservation
            }
            Self::ActiveGenerationBusy => ReasonCode::ActiveGenerationBusy,
            Self::WorkerHandshakeFailed(_) => ReasonCode::WorkerHandshakeFailed,
            Self::WorkerIncompatibleVersion(_) => ReasonCode::WorkerIncompatibleVersion,
            Self::WorkerCorruptFrame(_) => ReasonCode::WorkerCorruptFrame,
            Self::WorkerTimeout(_) => ReasonCode::WorkerTimeout,
            Self::WorkerCrashed(_) => ReasonCode::WorkerCrashed,
            Self::Cancelled => ReasonCode::Cancelled,
            Self::PolicyDenied(_) => ReasonCode::PolicyDenied,
            Self::ScopeEscapeAttempt(_) => ReasonCode::ScopeEscapeAttempt,
            Self::GrantRevokedOrExpired => ReasonCode::GrantRevokedOrExpired,
            Self::ApprovalDeclined => ReasonCode::ApprovalDeclined,
            Self::InsufficientEvidence(_) => ReasonCode::InsufficientEvidence,
            Self::BudgetExhausted(_) => ReasonCode::BudgetExhausted,
            Self::AuditFailure(_) => ReasonCode::AuditFailure,
            Self::Io(_) => ReasonCode::IoError,
            Self::Database(_) => ReasonCode::DatabaseError,
        }
    }

    /// Return a redacted, control-character-escaped message suitable for logs and diagnostics.
    pub fn redacted_message(&self) -> String {
        redact_sensitive_text(&self.to_string())
    }
}

impl From<std::io::Error> for LoraError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(redact_sensitive_text(&err.to_string()))
    }
}

impl From<rusqlite::Error> for LoraError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Database(redact_sensitive_text(&err.to_string()))
    }
}

/// Redact secret patterns, canary credentials, and control characters from diagnostic strings.
pub fn redact_sensitive_text(input: &str) -> String {
    let mut sanitized = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            sanitized.push_str(&format!("\\x{:02x}", ch as u32));
        } else {
            sanitized.push(ch);
        }
    }

    for marker in [
        "CANARY_SECRET_",
        "sk-live-",
        "ghp_",
        "Bearer ",
        "PRIVATE KEY",
        "PASSWORD=",
        "SECRET_KEY=",
    ] {
        while let Some(idx) = sanitized.find(marker) {
            let end = sanitized[idx..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
                .map(|offset| idx + offset)
                .unwrap_or(sanitized.len());
            sanitized.replace_range(idx..end, "[REDACTED]");
        }
    }
    sanitized
}
