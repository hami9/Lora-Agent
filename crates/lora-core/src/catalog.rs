//! Subsequent catalog resolution, allowlisted HTTPS verification, script/pickle blocking,
//! and `Range` / `If-Range` / strong `ETag` resumable download verification (`docs/MODEL_MANAGEMENT.md`).

use crate::error::LoraError;
use serde::{Deserialize, Serialize};

pub const MAX_CATALOG_REDIRECTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogVariantDescriptor {
    pub model_id: String,
    pub immutable_revision: String,
    pub filename: String,
    pub size_bytes: u64,
    pub license: String,
    pub expected_sha256: String,
    pub strong_etag: String,
    pub download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadResumeCheckpoint {
    pub immutable_revision: String,
    pub expected_sha256: String,
    pub strong_etag: String,
    pub total_size_bytes: u64,
    pub bytes_received: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeDecision {
    AppendToPartial,
    DiscardPartialAndRestart,
}

/// Validate a catalog artifact descriptor and download URL (`docs/MODEL_MANAGEMENT.md`):
/// - Must use HTTPS
/// - Redirect count must not exceed `MAX_CATALOG_REDIRECTS`
/// - Rejects pickle, Python scripts, archives, and executable hooks (`.pkl`, `.pickle`, `.pt`, `.bin`, `.py`, `.sh`, `.exe`)
/// - Requires `.gguf` format and a non-empty immutable revision + 64-hex SHA-256 digest
pub fn validate_catalog_variant(
    descriptor: &CatalogVariantDescriptor,
    redirect_hops: u8,
) -> Result<(), LoraError> {
    if redirect_hops > MAX_CATALOG_REDIRECTS {
        return Err(LoraError::PolicyDenied(format!(
            "catalog download exceeded maximum redirect hops ({redirect_hops} > {MAX_CATALOG_REDIRECTS})"
        )));
    }
    if !descriptor.download_url.starts_with("https://") {
        return Err(LoraError::PolicyDenied(
            "catalog download URL must use HTTPS".to_string(),
        ));
    }
    if descriptor.immutable_revision.trim().is_empty()
        || descriptor.immutable_revision.eq_ignore_ascii_case("main")
        || descriptor.immutable_revision.eq_ignore_ascii_case("latest")
    {
        return Err(LoraError::PolicyDenied(
            "mutable catalog branch names ('main'/'latest') must be resolved to an immutable commit revision before download"
                .to_string(),
        ));
    }

    let lower_name = descriptor.filename.trim().to_ascii_lowercase();
    for forbidden_ext in [
        ".pkl", ".pickle", ".pt", ".pth", ".bin", ".py", ".sh", ".bat", ".ps1", ".exe", ".dll",
        ".so", ".zip", ".tar",
    ] {
        if lower_name.ends_with(forbidden_ext) {
            return Err(LoraError::PolicyDenied(format!(
                "refusing unsafe or executable catalog artifact '{}' (only verified .gguf weights are permitted)",
                descriptor.filename
            )));
        }
    }
    if !lower_name.ends_with(".gguf") {
        return Err(LoraError::IncompatibleModel(format!(
            "catalog artifact '{}' must be a .gguf file",
            descriptor.filename
        )));
    }
    if descriptor.expected_sha256.len() != 64
        || !descriptor
            .expected_sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        return Err(LoraError::MalformedArtifact(
            "catalog variant requires a valid 64-character hex SHA-256 digest".to_string(),
        ));
    }
    if descriptor.strong_etag.starts_with("W/") || descriptor.strong_etag.trim().is_empty() {
        return Err(LoraError::MalformedArtifact(
            "resumable catalog download requires a strong ETag (weak W/ ETags are rejected)"
                .to_string(),
        ));
    }
    Ok(())
}

/// Evaluate whether an interrupted download can safely append a `Range` / `If-Range` response
/// or must discard the partial `.part` file and restart (`docs/MODEL_MANAGEMENT.md`):
/// "Resume with Range/If-Range only if byte-range response and artifact identity match;
/// otherwise discard the partial response and restart."
pub fn evaluate_range_resume(
    checkpoint: &DownloadResumeCheckpoint,
    http_status: u16,
    response_etag: &str,
    response_revision: &str,
    content_range_start: Option<u64>,
) -> Result<ResumeDecision, LoraError> {
    if response_etag.starts_with("W/") {
        return Ok(ResumeDecision::DiscardPartialAndRestart);
    }
    if http_status == 206
        && response_etag == checkpoint.strong_etag
        && response_revision == checkpoint.immutable_revision
        && content_range_start == Some(checkpoint.bytes_received)
    {
        Ok(ResumeDecision::AppendToPartial)
    } else {
        Ok(ResumeDecision::DiscardPartialAndRestart)
    }
}
