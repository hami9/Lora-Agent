//! OS-level platform adapters implementing the narrow ports defined in `lora-core`.

pub mod fs_executor;
pub mod hardware;
pub mod lock;
pub mod supervisor;

pub use fs_executor::{
    is_windows_reserved_device_name, validate_and_split_relative_path, OsScopedFileExecutor,
};
pub use hardware::SysinfoHardwareProbe;
pub use lock::{OsFileLockGuard, OsStoreLockPort};
pub use supervisor::{
    generate_launch_nonce, synthesize_deterministic_response, SubprocessWorkerSession,
    SubprocessWorkerSpawner,
};

use lora_core::{
    LoraError, WorkerGenerateParams, WorkerGenerationOutcome, WorkerHandshake, WorkerSessionPort,
    WorkerSpawnerPort,
};
use lora_protocol::{
    parse_gguf_file, sha256_file, GenerationUsage, ProbedModelMetadata, WorkerCapabilities,
    PINNED_LLAMA_CPP_REVISION,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Fault injection modes for testing worker failure handling (`T4`, `M4`, `M7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerFaultInjection {
    #[default]
    None,
    CorruptFrameOnLoad,
    IncompatibleVersionOnHandshake,
    CrashOnLoad,
    CrashMidGeneration,
    TimeoutOnStartup,
}

#[derive(Default, Clone)]
pub struct InProcessWorkerSpawner {
    pub fault: WorkerFaultInjection,
    pub per_token_delay_ms: u64,
}

struct InProcessWorkerSession {
    handshake: WorkerHandshake,
    fault: WorkerFaultInjection,
    per_token_delay_ms: u64,
    loaded_handle: Option<String>,
}

impl WorkerSpawnerPort for InProcessWorkerSpawner {
    fn spawn_worker(&self) -> Result<Box<dyn WorkerSessionPort>, LoraError> {
        match self.fault {
            WorkerFaultInjection::IncompatibleVersionOnHandshake => {
                return Err(LoraError::WorkerIncompatibleVersion(
                    "worker offered major 99, host requires major 1".to_string(),
                ));
            }
            WorkerFaultInjection::TimeoutOnStartup => {
                return Err(LoraError::WorkerTimeout(
                    "worker exceeded startup handshake deadline".to_string(),
                ));
            }
            _ => {}
        }

        Ok(Box::new(InProcessWorkerSession {
            handshake: WorkerHandshake {
                engine_name: "llama.cpp-shim".to_string(),
                engine_revision: PINNED_LLAMA_CPP_REVISION.to_string(),
                build_options: vec!["cpu".to_string(), "avx2".to_string()],
                capabilities: WorkerCapabilities::default(),
            },
            fault: self.fault,
            per_token_delay_ms: self.per_token_delay_ms,
            loaded_handle: None,
        }))
    }
}

impl WorkerSessionPort for InProcessWorkerSession {
    fn handshake(&self) -> &WorkerHandshake {
        &self.handshake
    }

    fn probe_artifact(
        &mut self,
        _artifact_id: &str,
        artifact_path: &Path,
        expected_sha256: &str,
    ) -> Result<ProbedModelMetadata, LoraError> {
        let actual = sha256_file(artifact_path)?;
        if !expected_sha256.is_empty() && actual != expected_sha256.to_ascii_lowercase() {
            return Err(LoraError::DigestMismatch {
                expected: expected_sha256.to_string(),
                actual,
            });
        }
        let parsed = parse_gguf_file(artifact_path)
            .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
        Ok(parsed.metadata)
    }

    fn load_model(
        &mut self,
        artifact_id: &str,
        artifact_path: &Path,
        expected_sha256: &str,
        context_length: u32,
        _threads: u32,
    ) -> Result<String, LoraError> {
        match self.fault {
            WorkerFaultInjection::CorruptFrameOnLoad => {
                return Err(LoraError::WorkerCorruptFrame(
                    "invalid frame length or schema during load_model".to_string(),
                ));
            }
            WorkerFaultInjection::CrashOnLoad => {
                return Err(LoraError::WorkerCrashed(
                    "worker exited unexpectedly with signal/code during load_model".to_string(),
                ));
            }
            _ => {}
        }

        let meta = self.probe_artifact(artifact_id, artifact_path, expected_sha256)?;
        if context_length > meta.context_length {
            return Err(LoraError::IncompatibleModel(format!(
                "requested context {context_length} exceeds model limit {}",
                meta.context_length
            )));
        }
        let handle = format!("mdl-{artifact_id}");
        self.loaded_handle = Some(handle.clone());
        Ok(handle)
    }

    fn generate_stream(
        &mut self,
        params: &WorkerGenerateParams,
        cancel_flag: &Arc<AtomicBool>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), LoraError>,
    ) -> Result<WorkerGenerationOutcome, LoraError> {
        if self.loaded_handle.as_deref() != Some(&params.model_handle) {
            return Err(LoraError::WorkerCrashed(
                "invalid or unloaded model handle".to_string(),
            ));
        }

        if cancel_flag.load(Ordering::SeqCst) {
            return Err(LoraError::Cancelled);
        }

        if self.fault == WorkerFaultInjection::CrashMidGeneration {
            on_delta("Partial token before crash ")?;
            return Err(LoraError::WorkerCrashed(
                "worker process terminated mid-decode".to_string(),
            ));
        }

        let (chunks, proposal, reason) =
            synthesize_deterministic_response(&params.prompt, params.tool_calling_enabled);

        let delay = params.simulated_delay_ms.max(self.per_token_delay_ms);
        let mut full_text = String::new();
        let mut emitted_tokens = 0u32;

        for chunk in chunks {
            if cancel_flag.load(Ordering::SeqCst) {
                return Err(LoraError::Cancelled);
            }
            if delay > 0 {
                thread::sleep(Duration::from_millis(delay));
                if cancel_flag.load(Ordering::SeqCst) {
                    return Err(LoraError::Cancelled);
                }
            }
            full_text.push_str(&chunk);
            emitted_tokens += 1;
            on_delta(&chunk)?;
        }

        Ok(WorkerGenerationOutcome {
            full_text,
            finish_reason: reason,
            usage: GenerationUsage {
                prompt_tokens: (params.prompt.len() / 4).max(1) as u32,
                completion_tokens: emitted_tokens,
                prefill_ms: 5,
                decode_ms: emitted_tokens as u64 * delay.max(1),
                peak_rss_bytes: 64 * 1024 * 1024,
            },
            tool_proposal: proposal,
        })
    }

    fn unload_model(&mut self, _model_handle: &str) -> Result<(), LoraError> {
        self.loaded_handle = None;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), LoraError> {
        self.loaded_handle = None;
        Ok(())
    }
}
