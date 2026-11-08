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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_sha256: Option<String>,
}

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Compute domain-separated cryptographic attestation digest for a `CompensatingPatchReceipt` (`lora.patch.receipt.v1`).
pub fn compute_patch_receipt_digest(receipt: &CompensatingPatchReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lora.patch.receipt.v1\0");
    update_len_prefixed(&mut hasher, receipt.relative_path.as_bytes());
    update_len_prefixed(
        &mut hasher,
        receipt.before_sha256.to_ascii_lowercase().as_bytes(),
    );
    update_len_prefixed(
        &mut hasher,
        receipt.after_sha256.to_ascii_lowercase().as_bytes(),
    );
    update_len_prefixed(&mut hasher, &receipt.before_bytes);
    hex::encode(hasher.finalize())
}

/// Verify self-integrity of a `CompensatingPatchReceipt` before attempting rollback:
/// 1. Rejects invalid or escaping relative paths (traversal, absolute/UNC, reserved names).
/// 2. Validates format of `before_sha256` and `after_sha256` (64-character hex).
/// 3. Fails closed with `LoraError::DigestMismatch` if `before_bytes` does not match `before_sha256`.
/// 4. Fails closed with `LoraError::DigestMismatch` if `receipt_sha256` is present and does not match the canonical digest.
pub fn verify_compensating_patch_receipt(
    receipt: &CompensatingPatchReceipt,
) -> Result<(), LoraError> {
    if is_excluded_secret_resource(&receipt.relative_path) {
        return Err(LoraError::PolicyDenied(format!(
            "cannot rollback secret-bearing resource '{}'",
            receipt.relative_path
        )));
    }
    crate::agent::validate_workflow_relative_path(&receipt.relative_path).map_err(|reason| {
        LoraError::ScopeEscapeAttempt(format!(
            "invalid relative path in compensating receipt '{}': {reason}",
            receipt.relative_path
        ))
    })?;

    if receipt.before_sha256.len() != 64
        || !receipt.before_sha256.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(LoraError::DigestMismatch {
            expected: "64-character hex before_sha256".to_string(),
            actual: receipt.before_sha256.clone(),
        });
    }

    if receipt.after_sha256.len() != 64
        || !receipt.after_sha256.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(LoraError::DigestMismatch {
            expected: "64-character hex after_sha256".to_string(),
            actual: receipt.after_sha256.clone(),
        });
    }

    let actual_before = hex::encode(Sha256::digest(&receipt.before_bytes));
    if !receipt.before_sha256.eq_ignore_ascii_case(&actual_before) {
        return Err(LoraError::DigestMismatch {
            expected: receipt.before_sha256.clone(),
            actual: actual_before,
        });
    }

    if let Some(ref r_digest) = receipt.receipt_sha256 {
        let expected_digest = compute_patch_receipt_digest(receipt);
        if !r_digest.eq_ignore_ascii_case(&expected_digest) {
            return Err(LoraError::DigestMismatch {
                expected: expected_digest,
                actual: r_digest.clone(),
            });
        }
    }

    Ok(())
}

fn resolve_checked_regular_file(
    root: &ScopedRootHandle,
    relative_path: &str,
) -> Result<PathBuf, LoraError> {
    if is_excluded_secret_resource(relative_path) {
        return Err(LoraError::PolicyDenied(format!(
            "cannot patch secret-bearing resource '{relative_path}'"
        )));
    }
    let components =
        crate::agent::validate_workflow_relative_path(relative_path).map_err(|reason| {
            LoraError::ScopeEscapeAttempt(format!(
                "invalid relative path for patch tool '{relative_path}': {reason}"
            ))
        })?;

    let mut current = root.canonical_root.clone();
    for (idx, comp) in components.iter().enumerate() {
        current.push(comp);
        let sym = fs::symlink_metadata(&current).map_err(|e| {
            LoraError::ScopeEscapeAttempt(format!(
                "cannot stat patch target component '{comp}' in '{relative_path}': {e}"
            ))
        })?;
        if sym.file_type().is_symlink() {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "patch target component '{comp}' in '{relative_path}' is a symlink (symlinks rejected)"
            )));
        }
        let is_last = idx + 1 == components.len();
        if !is_last && !sym.is_dir() {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "patch target component '{comp}' in '{relative_path}' is not a directory"
            )));
        }
        if is_last && !sym.is_file() {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "patch target '{relative_path}' must be an existing regular file (no symlinks)"
            )));
        }
    }
    let canonical = current.canonicalize().map_err(|e| {
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
    let tmp_path = {
        let mut p = target.as_os_str().to_os_string();
        p.push(".lora-patch.tmp");
        PathBuf::from(p)
    };
    fs::write(&tmp_path, updated_text.as_bytes())?;
    if let Err(e) = fs::rename(&tmp_path, &target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(LoraError::Io(format!(
            "failed to rename atomic patch temp file: {e}"
        )));
    }

    let mut receipt = CompensatingPatchReceipt {
        relative_path: proposal.relative_path.clone(),
        before_sha256: actual_before_sha,
        after_sha256,
        before_bytes,
        receipt_sha256: None,
    };
    receipt.receipt_sha256 = Some(compute_patch_receipt_digest(&receipt));
    Ok(receipt)
}

/// Compensating rollback operation: restores the exact `before_bytes` only if the target file
/// still matches `receipt.after_sha256` (`docs/AGENT_RUNTIME.md`).
pub fn rollback_applied_patch(
    root: &ScopedRootHandle,
    receipt: &CompensatingPatchReceipt,
) -> Result<(), LoraError> {
    verify_compensating_patch_receipt(receipt)?;
    let target = resolve_checked_regular_file(root, &receipt.relative_path)?;
    let current_bytes = fs::read(&target)?;
    let current_sha = hex::encode(Sha256::digest(&current_bytes));

    if current_sha != receipt.after_sha256 {
        return Err(LoraError::PolicyDenied(format!(
            "cannot auto-rollback '{}': file was modified after patch (expected sha256 {}, found {})",
            receipt.relative_path, receipt.after_sha256, current_sha
        )));
    }

    let tmp_path = {
        let mut p = target.as_os_str().to_os_string();
        p.push(".lora-rollback.tmp");
        PathBuf::from(p)
    };
    fs::write(&tmp_path, &receipt.before_bytes)?;
    if let Err(e) = fs::rename(&tmp_path, &target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(LoraError::Io(format!(
            "failed to rename atomic rollback temp file: {e}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_patch_receipt_digest_and_verification() {
        let rcpt = CompensatingPatchReceipt {
            relative_path: "src/main.rs".to_string(),
            before_sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                .to_string(),
            after_sha256: "48ae4e073d823ec18a38b1f5e88e7b99c0ae3214a1ecabdeab7c8e9b4d89a424"
                .to_string(),
            before_bytes: b"hello".to_vec(),
            receipt_sha256: None,
        };
        // Unmodified receipt verifies without error
        assert!(verify_compensating_patch_receipt(&rcpt).is_ok());

        // Binding receipt_sha256
        let digest = compute_patch_receipt_digest(&rcpt);
        assert_eq!(digest.len(), 64);
        let mut rcpt_with_digest = rcpt.clone();
        rcpt_with_digest.receipt_sha256 = Some(digest);
        assert!(verify_compensating_patch_receipt(&rcpt_with_digest).is_ok());

        // Tampered before_bytes fails closed with DigestMismatch
        let mut tampered = rcpt.clone();
        tampered.before_bytes = b"evil payload".to_vec();
        assert!(matches!(
            verify_compensating_patch_receipt(&tampered),
            Err(LoraError::DigestMismatch { .. })
        ));

        // Tampered receipt_sha256 fails closed
        let mut tampered_digest = rcpt_with_digest.clone();
        tampered_digest.receipt_sha256 = Some("0".repeat(64));
        assert!(matches!(
            verify_compensating_patch_receipt(&tampered_digest),
            Err(LoraError::DigestMismatch { .. })
        ));

        // Path traversal in receipt relative_path fails closed
        let mut bad_path = rcpt.clone();
        bad_path.relative_path = "../escape.rs".to_string();
        assert!(matches!(
            verify_compensating_patch_receipt(&bad_path),
            Err(LoraError::ScopeEscapeAttempt(_))
        ));

        // Secret resource in receipt relative_path denied by policy
        let mut secret_path = rcpt.clone();
        secret_path.relative_path = ".env".to_string();
        assert!(matches!(
            verify_compensating_patch_receipt(&secret_path),
            Err(LoraError::PolicyDenied(_))
        ));
    }

    #[test]
    fn test_patch_apply_and_rollback_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let canonical_root = tmp.path().canonicalize().unwrap();
        let root = ScopedRootHandle {
            root_id: "root-1".to_string(),
            canonical_root: canonical_root.clone(),
            device_or_volume_id: 1,
        };

        let file_path = canonical_root.join("config.ini");
        fs::write(&file_path, "key=value\n").unwrap();
        let before_sha = hex::encode(Sha256::digest(b"key=value\n"));

        let proposal = PatchProposal {
            root_id: root.root_id.clone(),
            relative_path: "config.ini".to_string(),
            expected_before_sha256: before_sha.clone(),
            old_snippet: "key=value".to_string(),
            new_snippet: "key=updated".to_string(),
        };

        let preview = preview_checked_patch(&root, &proposal).unwrap();
        assert_eq!(preview.before_sha256, before_sha);

        let receipt = apply_approved_patch(&root, &proposal, &preview.approval_digest).unwrap();
        assert!(receipt.receipt_sha256.is_some());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "key=updated\n");

        // Rollback restores original content
        rollback_applied_patch(&root, &receipt).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "key=value\n");
    }
}
