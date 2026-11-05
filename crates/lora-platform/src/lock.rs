//! OS-owned exclusive store lock (`docs/ARCHITECTURE.md`, `T2`, `T6`).
//!
//! Uses an OS file lock released automatically on process termination.
//! Also writes a diagnostic `.pid` file.

use lora_core::{LoraError, StoreLockGuard, StoreLockPort};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub struct OsFileLockGuard {
    _file: File,
}

impl StoreLockGuard for OsFileLockGuard {}

#[derive(Default, Clone)]
pub struct OsStoreLockPort;

impl StoreLockPort for OsStoreLockPort {
    fn acquire_lock(&self, data_dir: &Path) -> Result<Box<dyn StoreLockGuard>, LoraError> {
        fs::create_dir_all(data_dir)?;
        let lock_path = data_dir.join("store.lock");

        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            // ShareMode(0) enforces exclusive kernel-level access on Windows;
            // released automatically by the OS if the process exits or crashes.
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&lock_path)
                .map_err(|e| {
                    LoraError::StoreBusy(format!(
                        "failed to acquire exclusive lock on '{}': {e}",
                        lock_path.display()
                    ))
                })?
        };

        #[cfg(not(windows))]
        let file = {
            // Exclusive OS advisory lock (flock/fcntl) held by the open handle;
            // the OS releases it when the handle closes or the process exits.
            let busy = |e: &dyn std::fmt::Display| {
                LoraError::StoreBusy(format!(
                    "failed to acquire exclusive lock on '{}': {e}",
                    lock_path.display()
                ))
            };
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .map_err(|e| busy(&e))?;
            f.try_lock().map_err(|e| busy(&e))?;
            f
        };

        // Write diagnostic-only PID file
        let pid_path = data_dir.join("store.pid");
        if let Ok(mut pid_file) = File::create(pid_path) {
            let _ = writeln!(pid_file, "{}", std::process::id());
        }

        Ok(Box::new(OsFileLockGuard { _file: file }))
    }
}
