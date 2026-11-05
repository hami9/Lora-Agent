//! Supervised Agent checked patch tool with `before_image_sha256` precondition verification,
//! diff preview, and compensating rollback (`docs/AGENT_RUNTIME.md`, `docs/SECURITY_ARCHITECTURE.md`).

use crate::error::LoraError;
use crate::policy::is_excluded_secret_resource;
use crate::ports::ScopedRootHandle;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchProposal {
    pub root_id: String,
    pub relative_path: String,
    pub expected_before_sha256: String,
    pub old_snippet: String,
    pub new_snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchPreview {
    pub relative_path: String,
    pub before_sha256: String,
    pub projected_after_sha256: String,
    pub unified_diff_summary: String,
    pub approval_digest: String,
}

/// Compensating operation record allowing deterministic undo/rollback of an applied patch
/// if the file still matches `after_sha256` (`docs/AGENT_RUNTIME.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompensatingPatchReceipt {
    pub relative_path: String,
    pub before_sha256: String,
    pub after_sha256: String,
    pub before_bytes: Vec<u8>,
}

fn resolve_checked_regular_file(
    root: &ScopedRootHandle,
    relative_path: &str,
) -> Result<PathBuf, LoraError> {
    if relative_path.is_empty()
        || relative_path.contains("..")
        || relative_path.starts_with('/')
        || relative_path.starts_with('\\')
        || relative_path.contains(':')
        || relative_path.contains('\0')
    {
        return Err(LoraError::ScopeEscapeAttempt(format!(
            "invalid relative path for patch tool: '{relative_path}'"
        )));
    }
    if is_excluded_secret_resource(relative_path) {
        return Err(LoraError::PolicyDenied(format!(
            "cannot patch secret-bearing resource '{relative_path}'"
        )));
    }

    let full_path = root.canonical_root.join(relative_path);
    let sym = fs::symlink_metadata(&full_path).map_err(|e| {
        LoraError::ScopeEscapeAttempt(format!("cannot stat patch target '{relative_path}': {e}"))
    })?;
    if sym.file_type().is_symlink() || !sym.is_file() {
        return Err(LoraError::ScopeEscapeAttempt(format!(
            "patch target '{relative_path}' must be an existing regular file (no symlinks)"
        )));
    }
    let canonical = full_path.canonicalize().map_err(|e| {
        LoraError::ScopeEscapeAttempt(format!("cannot canonicalize patch target: {e}"))
    })?;
    if !canonical.starts_with(&root.canonical_root) {
        return Err(LoraError::ScopeEscapeAttempt(format!(
            "patch target '{relative_path}' escaped workspace root"
        )));
    }
    Ok(canonical)
}

/// Build an immutable diff preview and verify the `expected_before_sha256` precondition without mutating the file.
pub fn preview_checked_patch(
    root: &ScopedRootHandle,
    proposal: &PatchProposal,
) -> Result<PatchPreview, LoraError> {
    let target = resolve_checked_regular_file(root, &proposal.relative_path)?;
    let current_text = fs::read_to_string(&target)?;
    let actual_before_sha = hex::encode(Sha256::digest(current_text.as_bytes()));

    if !actual_before_sha.eq_ignore_ascii_case(&proposal.expected_before_sha256) {
        return Err(LoraError::PolicyDenied(format!(
            "precondition failed: file '{}' changed (expected sha256 {}, found {})",
            proposal.relative_path, proposal.expected_before_sha256, actual_before_sha
        )));
    }

    let matches_count = current_text.matches(&proposal.old_snippet).count();
    if matches_count != 1 {
        return Err(LoraError::MalformedArtifact(format!(
            "patch old_snippet must match exactly once in '{}' (found {} matches)",
            proposal.relative_path, matches_count
        )));
    }

    let projected_text = current_text.replacen(&proposal.old_snippet, &proposal.new_snippet, 1);
    let projected_after_sha256 = hex::encode(Sha256::digest(projected_text.as_bytes()));

    let unified_diff_summary = format!(
        "--- a/{path}\n+++ b/{path}\n-{old}\n+{new}",
        path = proposal.relative_path,
        old = proposal.old_snippet.replace('\n', "\n-"),
        new = proposal.new_snippet.replace('\n', "\n+"),
    );

    let mut hasher = Sha256::new();
    hasher.update(root.root_id.as_bytes());
    hasher.update(b":");
    hasher.update(proposal.relative_path.as_bytes());
    hasher.update(b":");
    hasher.update(actual_before_sha.as_bytes());
    hasher.update(b":");
    hasher.update(projected_after_sha256.as_bytes());
    let approval_digest = hex::encode(hasher.finalize());

    Ok(PatchPreview {
        relative_path: proposal.relative_path.clone(),
        before_sha256: actual_before_sha,
        projected_after_sha256,
        unified_diff_summary,
        approval_digest,
    })
}

/// Apply a one-time human-approved patch after re-verifying the `before_sha256` and `approval_digest` preconditions (`docs/SECURITY_ARCHITECTURE.md`).
/// Returns a `CompensatingPatchReceipt` that can roll back the change.
pub fn apply_approved_patch(
    root: &ScopedRootHandle,
    proposal: &PatchProposal,
    approved_digest: &str,
) -> Result<CompensatingPatchReceipt, LoraError> {
    let preview = preview_checked_patch(root, proposal)?;
    if preview.approval_digest != approved_digest {
        return Err(LoraError::PolicyDenied(
            "patch precondition or arguments changed after approval; one-time approval invalidated"
                .to_string(),
        ));
    }

    let target = resolve_checked_regular_file(root, &proposal.relative_path)?;
    let before_bytes = fs::read(&target)?;
    let current_text = String::from_utf8(before_bytes.clone())
        .map_err(|_| LoraError::PolicyDenied("non-UTF-8 file cannot be patched".to_string()))?;

    let actual_before_sha = hex::encode(Sha256::digest(&before_bytes));
    if actual_before_sha != preview.before_sha256 {
        return Err(LoraError::PolicyDenied(
            "concurrent modification detected immediately before write; aborting patch".to_string(),
        ));
    }

    let updated_text = current_text.replacen(&proposal.old_snippet, &proposal.new_snippet, 1);
    let after_sha256 = hex::encode(Sha256::digest(updated_text.as_bytes()));

    // Write via same-directory atomic temp file rename where possible
    let tmp_path = target.with_extension("lora-patch.tmp");
    fs::write(&tmp_path, updated_text.as_bytes())?;
    fs::rename(&tmp_path, &target)?;

    Ok(CompensatingPatchReceipt {
        relative_path: proposal.relative_path.clone(),
        before_sha256: actual_before_sha,
        after_sha256,
        before_bytes,
    })
}

/// Compensating rollback operation: restores the exact `before_bytes` only if the target file
/// still matches `receipt.after_sha256` (`docs/AGENT_RUNTIME.md`).
pub fn rollback_applied_patch(
    root: &ScopedRootHandle,
    receipt: &CompensatingPatchReceipt,
) -> Result<(), LoraError> {
    let target = resolve_checked_regular_file(root, &receipt.relative_path)?;
    let current_bytes = fs::read(&target)?;
    let current_sha = hex::encode(Sha256::digest(&current_bytes));

    if current_sha != receipt.after_sha256 {
        return Err(LoraError::PolicyDenied(format!(
            "cannot auto-rollback '{}': file was modified after patch (expected sha256 {}, found {})",
            receipt.relative_path, receipt.after_sha256, current_sha
        )));
    }

    let tmp_path = target.with_extension("lora-rollback.tmp");
    fs::write(&tmp_path, &receipt.before_bytes)?;
    fs::rename(&tmp_path, &target)?;
    Ok(())
}
