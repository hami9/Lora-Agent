//! Handle-verified, symlink/reparse-rejecting scoped file executor (`docs/OS_INTEGRATION.md`, `T7`, `M6`).

use lora_core::{LoraError, ScopedFileExecutorPort, ScopedReadOutput, ScopedRootHandle};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(windows)]
const FILE_ATTRIBUTE_DEVICE: u32 = 0x0040;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// Check whether a single path component is a reserved Windows device name (`CON`, `NUL`, `COM1`, etc.).
pub fn is_windows_reserved_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim();
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Validate a relative path string and split it into safe, normalized components (`docs/OS_INTEGRATION.md`).
/// Rejects parent traversal (`..`), absolute/UNC paths, colons/alternate data streams,
/// embedded NULs, trailing dots/spaces, and reserved Windows device names.
pub fn validate_and_split_relative_path(raw_path: &str) -> Result<Vec<String>, LoraError> {
    if raw_path.is_empty() || raw_path.len() > 512 {
        return Err(LoraError::ScopeEscapeAttempt(
            "relative path is empty or exceeds 512 bytes".to_string(),
        ));
    }
    if raw_path.contains('\0') {
        return Err(LoraError::ScopeEscapeAttempt(
            "relative path contains embedded NUL byte".to_string(),
        ));
    }
    if raw_path.starts_with('/')
        || raw_path.starts_with('\\')
        || raw_path.contains(':')
        || raw_path.starts_with('~')
    {
        return Err(LoraError::ScopeEscapeAttempt(format!(
            "absolute, UNC, drive-prefixed, or alternate-data-stream path rejected: '{raw_path}'"
        )));
    }

    let normalized = raw_path.replace('\\', "/");
    let mut components = Vec::new();

    for part in normalized.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "path traversal or empty component '{}' rejected in '{}'",
                part, raw_path
            )));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "ambiguous trailing dot or space in component '{part}'"
            )));
        }
        if is_windows_reserved_device_name(part) {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "reserved device name '{part}' rejected"
            )));
        }
        components.push(part.to_string());
    }

    if components.is_empty() || components.len() > 32 {
        return Err(LoraError::ScopeEscapeAttempt(
            "path component count out of bounds".to_string(),
        ));
    }

    Ok(components)
}

fn check_metadata_no_symlink_or_reparse(
    meta: &fs::Metadata,
    display: &str,
) -> Result<(), LoraError> {
    if meta.file_type().is_symlink() {
        return Err(LoraError::ScopeEscapeAttempt(format!(
            "symbolic link rejected along scoped path: {display}"
        )));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let attrs = meta.file_attributes();
        if (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "Windows reparse point or junction rejected along scoped path: {display}"
            )));
        }
        if (attrs & FILE_ATTRIBUTE_DEVICE) != 0 {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "Windows device file rejected: {display}"
            )));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 && meta.is_file() {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "multiply-linked hardlink file rejected in strict profile: {display}"
            )));
        }
    }
    Ok(())
}

#[derive(Default, Clone)]
pub struct OsScopedFileExecutor;

impl ScopedFileExecutorPort for OsScopedFileExecutor {
    fn register_root(&self, root_dir: &Path) -> Result<ScopedRootHandle, LoraError> {
        let sym_meta = fs::symlink_metadata(root_dir).map_err(|e| {
            LoraError::ScopeEscapeAttempt(format!("cannot stat workspace root directory: {e}"))
        })?;
        check_metadata_no_symlink_or_reparse(&sym_meta, &root_dir.to_string_lossy())?;
        if !sym_meta.is_dir() {
            return Err(LoraError::ScopeEscapeAttempt(
                "workspace root must be a regular directory".to_string(),
            ));
        }

        let canonical_root = root_dir.canonicalize().map_err(|e| {
            LoraError::ScopeEscapeAttempt(format!("cannot canonicalize workspace root: {e}"))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(canonical_root.to_string_lossy().as_bytes());
        let digest = hex::encode(hasher.finalize());
        let root_id = format!("root-{}", &digest[..12]);

        Ok(ScopedRootHandle {
            root_id,
            canonical_root,
            device_or_volume_id: 1,
        })
    }

    fn read_scoped_file(
        &self,
        root: &ScopedRootHandle,
        relative_path: &str,
        max_bytes: usize,
        recheck_grant: &dyn Fn() -> Result<(), LoraError>,
    ) -> Result<ScopedReadOutput, LoraError> {
        recheck_grant()?;

        let components = validate_and_split_relative_path(relative_path)?;
        let normalized_rel = components.join("/");

        // Verify root directory itself has not been replaced with a symlink/reparse point
        let root_meta = fs::symlink_metadata(&root.canonical_root).map_err(|e| {
            LoraError::ScopeEscapeAttempt(format!("workspace root became inaccessible: {e}"))
        })?;
        check_metadata_no_symlink_or_reparse(&root_meta, "workspace_root")?;

        // Walk each path component without following links or crossing reparse boundaries
        let mut current_path: PathBuf = root.canonical_root.clone();
        for (idx, comp) in components.iter().enumerate() {
            current_path.push(comp);
            let meta = fs::symlink_metadata(&current_path).map_err(|e| {
                LoraError::ScopeEscapeAttempt(format!(
                    "cannot access path component '{comp}' in '{normalized_rel}': {e}"
                ))
            })?;
            check_metadata_no_symlink_or_reparse(&meta, &normalized_rel)?;

            let is_last = idx + 1 == components.len();
            if !is_last && !meta.is_dir() {
                return Err(LoraError::ScopeEscapeAttempt(format!(
                    "intermediate component '{comp}' is not a directory"
                )));
            }
            if is_last && !meta.is_file() {
                return Err(LoraError::ScopeEscapeAttempt(format!(
                    "target '{normalized_rel}' is not a regular file"
                )));
            }
        }

        // Open the final file read-only and inspect the opened handle's metadata
        let file = File::open(&current_path).map_err(|e| {
            LoraError::ScopeEscapeAttempt(format!(
                "failed to open regular file '{normalized_rel}': {e}"
            ))
        })?;
        let handle_meta = file.metadata()?;
        check_metadata_no_symlink_or_reparse(&handle_meta, &normalized_rel)?;
        if !handle_meta.is_file() {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "opened handle for '{normalized_rel}' is not a regular file"
            )));
        }

        // Verify canonical path of target remains strictly inside `root.canonical_root`
        // (protects against rename-race substitution during open)
        let target_canonical = current_path.canonicalize().map_err(|e| {
            LoraError::ScopeEscapeAttempt(format!(
                "failed to verify canonical target identity for '{normalized_rel}': {e}"
            ))
        })?;
        if !target_canonical.starts_with(&root.canonical_root) {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "target '{normalized_rel}' resolved outside approved workspace root"
            )));
        }

        // Read at most `max_bytes + 1` to detect truncation without unbounded memory allocation
        let cap = max_bytes.max(1);
        let mut raw_buf = Vec::with_capacity(cap.min(64 * 1024) + 1);
        file.take((cap as u64) + 1).read_to_end(&mut raw_buf)?;

        let mut truncated = false;
        if raw_buf.len() > cap {
            raw_buf.truncate(cap);
            truncated = true;
        }

        // Reject binary inputs containing NUL bytes (`docs/AGENT_RUNTIME.md`)
        if raw_buf.contains(&0u8) {
            return Err(LoraError::PolicyDenied(format!(
                "binary file input rejected for '{normalized_rel}'"
            )));
        }

        // If truncated at a multi-byte UTF-8 sequence boundary, trim back to the last valid UTF-8 char boundary
        let valid_text = match std::str::from_utf8(&raw_buf) {
            Ok(s) => s.to_string(),
            Err(utf8_err) if truncated && utf8_err.error_len().is_none() => {
                let valid_up_to = utf8_err.valid_up_to();
                raw_buf.truncate(valid_up_to);
                String::from_utf8(raw_buf.clone()).map_err(|_| {
                    LoraError::PolicyDenied(format!(
                        "non-UTF-8 content rejected in '{normalized_rel}'"
                    ))
                })?
            }
            Err(_) => {
                return Err(LoraError::PolicyDenied(format!(
                    "non-UTF-8 binary content rejected in '{normalized_rel}'"
                )))
            }
        };

        // Recheck grant epoch right before releasing bytes to caller (`docs/OS_INTEGRATION.md`)
        recheck_grant()?;

        let mut hasher = Sha256::new();
        hasher.update(&raw_buf);
        let sha256_digest = hex::encode(hasher.finalize());

        let line_range = if valid_text.is_empty() {
            None
        } else {
            let line_count = valid_text.lines().count().max(1) as u32;
            Some((1, line_count))
        };

        let resource_identity = format!(
            "{}:size={}:sha256={}",
            normalized_rel,
            handle_meta.len(),
            &sha256_digest[..16]
        );

        Ok(ScopedReadOutput {
            root_id: root.root_id.clone(),
            relative_path: normalized_rel,
            resource_identity,
            content: valid_text,
            byte_start: 0,
            byte_end: raw_buf.len() as u64,
            line_range,
            sha256_digest,
            truncated,
        })
    }
}
