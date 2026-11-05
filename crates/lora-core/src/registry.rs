//! Model catalog, Manifest v1, crash-safe staging, and content-addressed blob store (`docs/MODEL_MANAGEMENT.md`, `T3`, `M1`, `M2`, `M7`).

use crate::error::LoraError;
use crate::ports::{HardwareProbePort, WorkerSpawnerPort};
use crate::store::{now_epoch_ms, ModelLifecycleState, SqliteStore};
use lora_protocol::{parse_gguf_file, ProbedModelMetadata, PINNED_LLAMA_CPP_REVISION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFileEntry {
    pub role: String,
    pub expected_size: u64,
    pub sha256: String,
    pub format: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityRecord {
    pub engine_name: String,
    pub engine_revision: String,
    pub backend: String,
    pub qualified: bool,
    pub tested_max_context: u32,
}

/// Model Manifest v1 (`docs/MODEL_MANAGEMENT.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifestV1 {
    pub schema_major: u16,
    pub schema_minor: u16,
    pub id: String,
    pub revision: String,
    pub display_alias: String,
    pub source: String,
    pub source_revision: String,
    pub retrieved_at_ms: u64,
    pub files: Vec<ManifestFileEntry>,
    pub probed: ProbedModelMetadata,
    pub license: String,
    pub provenance_authenticated: bool,
    pub compatibility: CompatibilityRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Imported,
    DuplicateExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub status: ImportStatus,
    pub manifest: ModelManifestV1,
    pub blob_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteOutcome {
    pub revision_id: String,
    pub display_alias: String,
    pub sha256: String,
    pub final_state: ModelLifecycleState,
    pub blob_garbage_collected: bool,
}

/// Classify a candidate local model filename against the `docs/INFERENCE_ENGINE.md` compatibility matrix:
/// - Single verified `.gguf` dense text artifact -> `Ok(())`
/// - `.safetensors`, `.onnx`, sharded `-00001-of-` GGUFs, multimodal projector (`mmproj`), or LoRA adapter -> typed `LoraError::IncompatibleModel`
pub fn classify_artifact_compatibility(filename: &str) -> Result<(), LoraError> {
    let lower = filename.trim().to_ascii_lowercase();
    if lower.ends_with(".safetensors") {
        return Err(LoraError::IncompatibleModel(
            "HF safetensors bundle is unsupported for direct execution; no automatic conversion is performed".to_string(),
        ));
    }
    if lower.ends_with(".onnx") {
        return Err(LoraError::IncompatibleModel(
            "ONNX model format is unsupported by the GGUF worker".to_string(),
        ));
    }
    if lower.contains("mmproj") || lower.contains("projector") {
        return Err(LoraError::IncompatibleModel(
            "multimodal GGUF projector inputs require dedicated media decoder isolation and are unsupported in text-only mode".to_string(),
        ));
    }
    if lower.contains("-of-00") || lower.contains("-00001-of-") {
        return Err(LoraError::IncompatibleModel(
            "sharded multi-file GGUF inputs are unsupported in single-file mode".to_string(),
        ));
    }
    if lower.contains("lora-adapter") || lower.ends_with(".adapter.gguf") {
        return Err(LoraError::IncompatibleModel(
            "LoRA adapter artifact cannot be imported as a standalone base model".to_string(),
        ));
    }
    if !lower.ends_with(".gguf") {
        return Err(LoraError::IncompatibleModel(format!(
            "unsupported model file extension in '{filename}' (expected single-file .gguf)"
        )));
    }
    Ok(())
}

pub struct ModelRegistry {
    store: SqliteStore,
    blobs_dir: PathBuf,
    staging_dir: PathBuf,
}

impl ModelRegistry {
    pub fn new(store: SqliteStore, data_dir: &Path) -> Result<Self, LoraError> {
        let blobs_dir = data_dir.join("blobs").join("sha256");
        let staging_dir = data_dir.join("staging");
        fs::create_dir_all(&blobs_dir)?;
        fs::create_dir_all(&staging_dir)?;
        Ok(Self {
            store,
            blobs_dir,
            staging_dir,
        })
    }

    pub fn blobs_dir(&self) -> &Path {
        &self.blobs_dir
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    pub fn blob_path_for_digest(&self, sha256: &str) -> PathBuf {
        self.blobs_dir.join(format!("{sha256}.gguf"))
    }

    /// Crash-safe offline model import (`docs/MODEL_MANAGEMENT.md`):
    /// 1. Check source regular file & free disk space for staging + final blob (`M7`).
    /// 2. Stream to private temporary staging file while hashing SHA-256.
    /// 3. Verify expected digest if provided (`M2`).
    /// 4. Validate bounded GGUF header/tensors and probe via isolated worker (`M2`).
    /// 5. Atomically promote blob to `blobs/sha256/<digest>.gguf` and commit `Ready` in SQLite.
    pub fn import_local_gguf(
        &self,
        source_path: &Path,
        alias: Option<&str>,
        expected_sha256: Option<&str>,
        hw_probe: &dyn HardwareProbePort,
        worker_spawner: &dyn WorkerSpawnerPort,
    ) -> Result<ImportOutcome, LoraError> {
        let meta = fs::symlink_metadata(source_path).map_err(|e| {
            LoraError::MalformedArtifact(format!("cannot stat source model path: {e}"))
        })?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(LoraError::MalformedArtifact(
                "source model must be a regular file (symlinks and special files are rejected)"
                    .to_string(),
            ));
        }

        let file_size = meta.len();
        if file_size < 24 {
            return Err(LoraError::MalformedArtifact(
                "file is smaller than minimum 24-byte GGUF header".to_string(),
            ));
        }

        // Check available disk space for staging + final blob copy
        let host_snap = hw_probe.capture_snapshot(&self.staging_dir)?;
        let required_disk = file_size.saturating_mul(2);
        if host_snap.free_disk_bytes < required_disk {
            return Err(LoraError::DiskFull {
                required_bytes: required_disk,
                available_bytes: host_snap.free_disk_bytes,
            });
        }

        let display_name = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.gguf")
            .to_string();
        let display_alias = alias
            .map(|a| a.to_string())
            .unwrap_or_else(|| display_name.trim_end_matches(".gguf").to_string());

        let temp_id = format!("import-{}-{}", std::process::id(), now_epoch_ms());
        let staging_path = self.staging_dir.join(format!("{temp_id}.part"));

        self.store
            .insert_model_staging(&temp_id, &display_alias, "pending", file_size)?;

        let copy_res = (|| -> Result<String, LoraError> {
            let mut src = File::open(source_path)?;
            let mut dst = File::create(&staging_path)?;
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 64 * 1024];
            let mut copied = 0u64;

            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                copied += n as u64;
                if copied > file_size {
                    return Err(LoraError::MalformedArtifact(
                        "source file grew during import copy".to_string(),
                    ));
                }
                hasher.update(&buf[..n]);
                dst.write_all(&buf[..n])?;
            }
            dst.sync_all()?;
            if copied != file_size {
                return Err(LoraError::MalformedArtifact(
                    "source file was truncated during import copy".to_string(),
                ));
            }
            let actual_sha = hex::encode(hasher.finalize());
            Ok(actual_sha)
        })();

        let actual_sha256 = match copy_res {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_file(&staging_path);
                let _ =
                    self.store
                        .update_model_state(&temp_id, ModelLifecycleState::Rejected, None);
                return Err(e);
            }
        };

        self.store
            .update_model_state(&temp_id, ModelLifecycleState::Verifying, None)?;

        if let Some(expected) = expected_sha256 {
            let normalized = expected.trim().to_ascii_lowercase();
            if normalized != actual_sha256 {
                let _ = fs::remove_file(&staging_path);
                let _ =
                    self.store
                        .update_model_state(&temp_id, ModelLifecycleState::Rejected, None);
                return Err(LoraError::DigestMismatch {
                    expected: normalized,
                    actual: actual_sha256,
                });
            }
        }

        // Check if this exact content digest is already registered and Ready
        if let Some((_existing_rev, _existing_alias, manifest_json)) =
            self.store.find_ready_model_by_sha256(&actual_sha256)?
        {
            let _ = fs::remove_file(&staging_path);
            let _ = self
                .store
                .update_model_state(&temp_id, ModelLifecycleState::Collected, None);
            let manifest: ModelManifestV1 = serde_json::from_str(&manifest_json)
                .map_err(|e| LoraError::Database(e.to_string()))?;
            let blob_path = self.blob_path_for_digest(&actual_sha256);
            return Ok(ImportOutcome {
                status: ImportStatus::DuplicateExisting,
                manifest,
                blob_path,
            });
        }

        // Parse the GGUF header from the staged copy (header region only) and validate bounds
        let parsed = match parse_gguf_file(&staging_path) {
            Ok(p) => p,
            Err(gguf_err) => {
                let _ = fs::remove_file(&staging_path);
                let _ =
                    self.store
                        .update_model_state(&temp_id, ModelLifecycleState::Rejected, None);
                return Err(LoraError::MalformedArtifact(gguf_err.to_string()));
            }
        };

        self.store
            .update_model_state(&temp_id, ModelLifecycleState::Probing, None)?;

        // Reconcile with isolated worker probe
        let probed = {
            let mut worker = match worker_spawner.spawn_worker() {
                Ok(w) => w,
                Err(e) => {
                    let _ = fs::remove_file(&staging_path);
                    let _ = self.store.update_model_state(
                        &temp_id,
                        ModelLifecycleState::Rejected,
                        None,
                    );
                    return Err(e);
                }
            };
            let res = worker.probe_artifact(&temp_id, &staging_path, &actual_sha256);
            let _ = worker.shutdown();
            match res {
                Ok(m) => m,
                Err(e) => {
                    let _ = fs::remove_file(&staging_path);
                    let _ = self.store.update_model_state(
                        &temp_id,
                        ModelLifecycleState::Rejected,
                        None,
                    );
                    return Err(e);
                }
            }
        };

        if probed.architecture != parsed.metadata.architecture
            || probed.template_digest != parsed.metadata.template_digest
        {
            let _ = fs::remove_file(&staging_path);
            let _ = self
                .store
                .update_model_state(&temp_id, ModelLifecycleState::Rejected, None);
            return Err(LoraError::IncompatibleModel(
                "worker probe metadata disagreed with host GGUF header parser".to_string(),
            ));
        }

        // Promote staged file to content-addressed blob path
        let final_blob_path = self.blob_path_for_digest(&actual_sha256);
        if !final_blob_path.exists() {
            if let Err(e) = fs::rename(&staging_path, &final_blob_path) {
                let _ = fs::remove_file(&staging_path);
                let _ =
                    self.store
                        .update_model_state(&temp_id, ModelLifecycleState::Rejected, None);
                return Err(LoraError::Io(format!("failed atomic blob promotion: {e}")));
            }
        } else {
            let _ = fs::remove_file(&staging_path);
        }

        let revision_id = format!("rev-{}", &actual_sha256[..16]);
        let manifest = ModelManifestV1 {
            schema_major: 1,
            schema_minor: 0,
            id: display_alias.clone(),
            revision: revision_id.clone(),
            display_alias: display_alias.clone(),
            source: "local_import".to_string(),
            source_revision: "user_supplied_unauthenticated".to_string(),
            retrieved_at_ms: now_epoch_ms(),
            files: vec![ManifestFileEntry {
                role: "weights".to_string(),
                expected_size: file_size,
                sha256: actual_sha256.clone(),
                format: "gguf".to_string(),
                display_name,
            }],
            probed: probed.clone(),
            license: probed.license.clone(),
            provenance_authenticated: expected_sha256.is_some(),
            compatibility: CompatibilityRecord {
                engine_name: "llama.cpp".to_string(),
                engine_revision: PINNED_LLAMA_CPP_REVISION.to_string(),
                backend: "cpu".to_string(),
                qualified: true,
                tested_max_context: probed.context_length,
            },
        };

        let manifest_json =
            serde_json::to_string(&manifest).map_err(|e| LoraError::Database(e.to_string()))?;

        // Update staging entry to Ready with final digest and manifest
        self.store
            .insert_model_staging(&revision_id, &display_alias, &actual_sha256, file_size)?;
        self.store.update_model_state(
            &revision_id,
            ModelLifecycleState::Ready,
            Some(&manifest_json),
        )?;
        self.store
            .update_model_state(&temp_id, ModelLifecycleState::Collected, None)?;

        Ok(ImportOutcome {
            status: ImportStatus::Imported,
            manifest,
            blob_path: final_blob_path,
        })
    }

    pub fn resolve_ready_model(
        &self,
        id_or_alias_or_sha: &str,
    ) -> Result<(ModelManifestV1, PathBuf), LoraError> {
        let Some((_rev_id, _alias, sha256, _size, manifest_json)) = self
            .store
            .find_ready_model_by_id_or_alias(id_or_alias_or_sha)?
        else {
            return Err(LoraError::IncompatibleModel(format!(
                "model '{id_or_alias_or_sha}' is not registered or not in Ready state"
            )));
        };

        let manifest: ModelManifestV1 =
            serde_json::from_str(&manifest_json).map_err(|e| LoraError::Database(e.to_string()))?;
        if manifest.schema_major != 1 {
            return Err(LoraError::IncompatibleModel(format!(
                "unsupported manifest schema_major {}",
                manifest.schema_major
            )));
        }

        let blob_path = self.blob_path_for_digest(&sha256);
        if !blob_path.is_file() {
            return Err(LoraError::MalformedArtifact(format!(
                "registered model blob missing on disk for digest {sha256}"
            )));
        }

        Ok((manifest, blob_path))
    }

    pub fn list_models(&self) -> Result<Vec<ModelManifestV1>, LoraError> {
        let rows = self.store.list_ready_models()?;
        let mut out = Vec::with_capacity(rows.len());
        for (_rev, _alias, _sha, _size, json) in rows {
            if let Ok(m) = serde_json::from_str::<ModelManifestV1>(&json) {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// Retire a model revision (`Ready -> Retired`) and garbage-collect its blob (`Retired -> Collected`)
    /// only if no active session holds a lease and no other `Ready` revision references the same SHA-256 digest
    /// (`docs/MODEL_MANAGEMENT.md`).
    pub fn retire_and_collect_model(
        &self,
        id_or_alias_or_sha: &str,
        has_active_lease: bool,
    ) -> Result<DeleteOutcome, LoraError> {
        if has_active_lease {
            return Err(LoraError::ActiveGenerationBusy);
        }

        let Some((rev_id, alias, sha256, _size, _manifest_json)) = self
            .store
            .find_ready_model_by_id_or_alias(id_or_alias_or_sha)?
        else {
            return Err(LoraError::IncompatibleModel(format!(
                "model '{id_or_alias_or_sha}' is not registered or not in Ready state"
            )));
        };

        // 1. Transition Ready -> Retired first so the alias is no longer resolvable
        self.store
            .update_model_state(&rev_id, ModelLifecycleState::Retired, None)?;

        // 2. Check if any remaining Ready model revision still references this SHA-256 blob
        let still_referenced = self.store.find_ready_model_by_sha256(&sha256)?.is_some();
        if still_referenced {
            return Ok(DeleteOutcome {
                revision_id: rev_id,
                display_alias: alias,
                sha256,
                final_state: ModelLifecycleState::Retired,
                blob_garbage_collected: false,
            });
        }

        // 3. Garbage-collect unreferenced blob file (`Retired -> Collected`)
        let blob_path = self.blob_path_for_digest(&sha256);
        let mut collected = false;
        if blob_path.exists() {
            if fs::remove_file(&blob_path).is_ok() {
                collected = true;
            }
        } else {
            collected = true;
        }

        let final_state = if collected {
            self.store
                .update_model_state(&rev_id, ModelLifecycleState::Collected, None)?;
            ModelLifecycleState::Collected
        } else {
            ModelLifecycleState::Retired
        };

        Ok(DeleteOutcome {
            revision_id: rev_id,
            display_alias: alias,
            sha256,
            final_state,
            blob_garbage_collected: collected,
        })
    }
}
