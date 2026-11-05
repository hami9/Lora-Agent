//! Worker process supervisor over private inherited anonymous pipes (`docs/INFERENCE_ENGINE.md`, `T4`, `M4`).
//!
//! Includes both `SubprocessWorkerSpawner` (launching the isolated `lora-llama-worker` binary)
//! and `ConfigurableMockWorkerSpawner` for deterministic fault-injection tests.

use lora_core::{
    now_epoch_ms, LoraError, WorkerGenerateParams, WorkerGenerationOutcome, WorkerHandshake,
    WorkerSessionPort, WorkerSpawnerPort,
};
use lora_protocol::{
    read_worker_event, validate_nonce, write_host_request, FinishReason, HostRequest,
    ProbedModelMetadata, ProtocolError, ProtocolVersion, ReadFileArgs, ToolProposalWire,
    WorkerCapabilities, WorkerErrorCode, WorkerEvent, PINNED_LLAMA_CPP_REVISION,
    SUPPORTED_PROTOCOL_MAJOR,
};
use sha2::{Digest, Sha256};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a fresh 128-bit lowercase hex nonce (`32` hex chars) for each worker launch.
pub fn generate_launch_nonce() -> String {
    let seq = NONCE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut hasher = Sha256::new();
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(now_epoch_ms().to_le_bytes());
    hasher.update(seq.to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    digest[..32].to_string()
}

/// Spawns the isolated `lora-llama-worker` executable over inherited anonymous pipes.
#[derive(Clone)]
pub struct SubprocessWorkerSpawner {
    worker_bin: PathBuf,
    extra_env: Vec<(String, String)>,
}

impl SubprocessWorkerSpawner {
    pub fn new(worker_bin: PathBuf) -> Self {
        Self {
            worker_bin,
            extra_env: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), val.into()));
        self
    }
}

pub struct SubprocessWorkerSession {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    handshake: WorkerHandshake,
    req_counter: u64,
}

impl SubprocessWorkerSession {
    fn next_req_id(&mut self, prefix: &str) -> String {
        self.req_counter += 1;
        format!("{prefix}-{}", self.req_counter)
    }

    fn kill_and_reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn map_protocol_err(&mut self, err: ProtocolError) -> LoraError {
        self.kill_and_reap();
        match err {
            ProtocolError::IncompatibleMajor { offered, required } => {
                LoraError::WorkerIncompatibleVersion(format!(
                    "worker offered majors {offered:?}, required {required}"
                ))
            }
            ProtocolError::InvalidNonce => LoraError::WorkerHandshakeFailed(
                "worker returned invalid or mismatched nonce".to_string(),
            ),
            ProtocolError::Frame(f) => LoraError::WorkerCorruptFrame(f.to_string()),
            ProtocolError::InvalidSchema(s) => LoraError::WorkerCorruptFrame(s),
            ProtocolError::InvalidStateTransition {
                message_type,
                state,
            } => LoraError::WorkerCorruptFrame(format!(
                "invalid message {message_type} in state {state}"
            )),
        }
    }

    fn map_worker_error(code: WorkerErrorCode, message: String) -> LoraError {
        match code {
            WorkerErrorCode::IncompatibleInput | WorkerErrorCode::UnsupportedCapability => {
                LoraError::IncompatibleModel(message)
            }
            WorkerErrorCode::InsufficientResources => LoraError::InsufficientMemory {
                explanation: message,
                estimated_weights_bytes: 0,
                estimated_kv_bytes: 0,
                available_headroom_bytes: 0,
                suggested_context_length: None,
            },
            WorkerErrorCode::InvalidWireVersion => LoraError::WorkerIncompatibleVersion(message),
            WorkerErrorCode::CorruptArtifact => LoraError::MalformedArtifact(message),
            WorkerErrorCode::Timeout => LoraError::WorkerTimeout(message),
            WorkerErrorCode::Cancelled => LoraError::Cancelled,
            WorkerErrorCode::OverContextLimit => LoraError::IncompatibleModel(message),
            WorkerErrorCode::InvalidState | WorkerErrorCode::InternalError => {
                LoraError::WorkerCrashed(message)
            }
        }
    }
}

impl Drop for SubprocessWorkerSession {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

impl WorkerSpawnerPort for SubprocessWorkerSpawner {
    fn spawn_worker(&self) -> Result<Box<dyn WorkerSessionPort>, LoraError> {
        if !self.worker_bin.is_file() {
            return Err(LoraError::WorkerHandshakeFailed(format!(
                "worker binary not found at '{}'",
                self.worker_bin.display()
            )));
        }

        let mut cmd = Command::new(&self.worker_bin);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Scrub inherited environment Secrets (`docs/SECURITY_ARCHITECTURE.md`)
            .env_clear();

        // Preserve minimal OS runtime loader variables only
        for safe_var in [
            "SystemRoot",
            "WINDIR",
            "PATH",
            "LD_LIBRARY_PATH",
            "DYLD_LIBRARY_PATH",
        ] {
            if let Ok(val) = std::env::var(safe_var) {
                cmd.env(safe_var, val);
            }
        }
        // Forward validated worker backend / log toggles across the scrubbed environment boundary
        if let Ok(backend) = std::env::var("LORA_WORKER_BACKEND") {
            let trimmed = backend.trim().to_ascii_lowercase();
            if matches!(trimmed.as_str(), "native" | "deterministic") {
                cmd.env("LORA_WORKER_BACKEND", trimmed);
            }
        }
        if let Ok(log_flag) = std::env::var("LORA_WORKER_LLAMA_LOG") {
            let trimmed = log_flag.trim();
            if matches!(trimmed, "0" | "1") {
                cmd.env("LORA_WORKER_LLAMA_LOG", trimmed);
            }
        }
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            LoraError::WorkerHandshakeFailed(format!("failed to spawn worker process: {e}"))
        })?;

        let stdin = BufWriter::new(child.stdin.take().ok_or_else(|| {
            LoraError::WorkerHandshakeFailed("missing worker stdin pipe".to_string())
        })?);
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| {
            LoraError::WorkerHandshakeFailed("missing worker stdout pipe".to_string())
        })?);

        let nonce = generate_launch_nonce();
        validate_nonce(&nonce).expect("generated nonce is valid");

        let mut session = SubprocessWorkerSession {
            child,
            stdin,
            stdout,
            handshake: WorkerHandshake {
                engine_name: String::new(),
                engine_revision: String::new(),
                build_options: Vec::new(),
                capabilities: WorkerCapabilities::default(),
            },
            req_counter: 0,
        };

        let hello = HostRequest::Hello {
            versions: vec![ProtocolVersion::V1_0],
            nonce: nonce.clone(),
        };
        if let Err(e) = write_host_request(&mut session.stdin, &hello) {
            return Err(session.map_protocol_err(e));
        }

        let reply = match read_worker_event(&mut session.stdout) {
            Ok(Some(ev)) => ev,
            Ok(None) => {
                session.kill_and_reap();
                return Err(LoraError::WorkerCrashed(
                    "worker exited before sending HelloAck".to_string(),
                ));
            }
            Err(e) => return Err(session.map_protocol_err(e)),
        };

        match reply {
            WorkerEvent::HelloAck {
                selected_version,
                nonce: ack_nonce,
                engine_name,
                engine_revision,
                build_options,
                capabilities,
            } => {
                if selected_version.major != SUPPORTED_PROTOCOL_MAJOR {
                    session.kill_and_reap();
                    return Err(LoraError::WorkerIncompatibleVersion(format!(
                        "worker selected unsupported major {}",
                        selected_version.major
                    )));
                }
                if ack_nonce != nonce {
                    session.kill_and_reap();
                    return Err(LoraError::WorkerHandshakeFailed(
                        "worker HelloAck nonce did not match launch nonce".to_string(),
                    ));
                }
                session.handshake = WorkerHandshake {
                    engine_name,
                    engine_revision,
                    build_options,
                    capabilities,
                };
                Ok(Box::new(session))
            }
            WorkerEvent::Error { code, message, .. } => {
                session.kill_and_reap();
                Err(SubprocessWorkerSession::map_worker_error(code, message))
            }
            other => {
                session.kill_and_reap();
                Err(LoraError::WorkerHandshakeFailed(format!(
                    "unexpected initial worker event: {other:?}"
                )))
            }
        }
    }
}

impl WorkerSessionPort for SubprocessWorkerSession {
    fn handshake(&self) -> &WorkerHandshake {
        &self.handshake
    }

    fn probe_artifact(
        &mut self,
        artifact_id: &str,
        artifact_path: &Path,
        expected_sha256: &str,
    ) -> Result<ProbedModelMetadata, LoraError> {
        let request_id = self.next_req_id("probe");
        let req = HostRequest::ProbeArtifact {
            request_id: request_id.clone(),
            artifact_id: artifact_id.to_string(),
            artifact_path: artifact_path.to_string_lossy().to_string(),
            expected_sha256: expected_sha256.to_string(),
        };
        if let Err(e) = write_host_request(&mut self.stdin, &req) {
            return Err(self.map_protocol_err(e));
        }

        match read_worker_event(&mut self.stdout) {
            Ok(Some(WorkerEvent::ProbeResult {
                request_id: ack_id,
                metadata,
            })) if ack_id == request_id => Ok(metadata),
            Ok(Some(WorkerEvent::Error { code, message, .. })) => {
                Err(Self::map_worker_error(code, message))
            }
            Ok(Some(other)) => {
                self.kill_and_reap();
                Err(LoraError::WorkerCorruptFrame(format!(
                    "unexpected event for probe_artifact: {other:?}"
                )))
            }
            Ok(None) => {
                self.kill_and_reap();
                Err(LoraError::WorkerCrashed(
                    "worker exited during probe_artifact".to_string(),
                ))
            }
            Err(e) => Err(self.map_protocol_err(e)),
        }
    }

    fn load_model(
        &mut self,
        artifact_id: &str,
        artifact_path: &Path,
        expected_sha256: &str,
        context_length: u32,
        threads: u32,
    ) -> Result<String, LoraError> {
        let request_id = self.next_req_id("load");
        let req = HostRequest::LoadModel {
            request_id: request_id.clone(),
            artifact_id: artifact_id.to_string(),
            artifact_path: artifact_path.to_string_lossy().to_string(),
            expected_sha256: expected_sha256.to_string(),
            context_length,
            threads,
        };
        if let Err(e) = write_host_request(&mut self.stdin, &req) {
            return Err(self.map_protocol_err(e));
        }

        match read_worker_event(&mut self.stdout) {
            Ok(Some(WorkerEvent::ModelLoaded {
                request_id: ack_id,
                model_handle,
                ..
            })) if ack_id == request_id => Ok(model_handle),
            Ok(Some(WorkerEvent::Error { code, message, .. })) => {
                Err(Self::map_worker_error(code, message))
            }
            Ok(Some(other)) => {
                self.kill_and_reap();
                Err(LoraError::WorkerCorruptFrame(format!(
                    "unexpected event for load_model: {other:?}"
                )))
            }
            Ok(None) => {
                self.kill_and_reap();
                Err(LoraError::WorkerCrashed(
                    "worker exited during load_model".to_string(),
                ))
            }
            Err(e) => Err(self.map_protocol_err(e)),
        }
    }

    fn generate_stream(
        &mut self,
        params: &WorkerGenerateParams,
        cancel_flag: &Arc<AtomicBool>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), LoraError>,
    ) -> Result<WorkerGenerationOutcome, LoraError> {
        let req = HostRequest::Generate {
            request_id: params.request_id.clone(),
            session_id: params.session_id.clone(),
            model_handle: params.model_handle.clone(),
            prompt: params.prompt.clone(),
            max_tokens: params.max_tokens,
            context_length: params.context_length,
            seed: params.seed,
            tool_calling_enabled: params.tool_calling_enabled,
            simulated_delay_ms: params.simulated_delay_ms,
        };
        if let Err(e) = write_host_request(&mut self.stdin, &req) {
            return Err(self.map_protocol_err(e));
        }

        let mut full_text = String::new();
        let mut last_seq = 0u64;
        let mut cancel_sent = false;

        loop {
            if cancel_flag.load(Ordering::SeqCst) && !cancel_sent {
                cancel_sent = true;
                let cancel_req = HostRequest::Cancel {
                    request_id: self.next_req_id("cancel"),
                    target_request_id: params.request_id.clone(),
                };
                let _ = write_host_request(&mut self.stdin, &cancel_req);
            }

            let ev = match read_worker_event(&mut self.stdout) {
                Ok(Some(e)) => e,
                Ok(None) => {
                    self.kill_and_reap();
                    return Err(LoraError::WorkerCrashed(
                        "worker exited mid-generation before terminal event".to_string(),
                    ));
                }
                Err(e) => return Err(self.map_protocol_err(e)),
            };

            match ev {
                WorkerEvent::GenerationDelta {
                    request_id,
                    sequence,
                    text_delta,
                    ..
                } => {
                    if request_id != params.request_id || sequence <= last_seq {
                        self.kill_and_reap();
                        return Err(LoraError::WorkerCorruptFrame(
                            "out-of-order or mismatched GenerationDelta".to_string(),
                        ));
                    }
                    last_seq = sequence;

                    if cancel_flag.load(Ordering::SeqCst) {
                        if !cancel_sent {
                            cancel_sent = true;
                            let cancel_req = HostRequest::Cancel {
                                request_id: self.next_req_id("cancel"),
                                target_request_id: params.request_id.clone(),
                            };
                            let _ = write_host_request(&mut self.stdin, &cancel_req);
                        }
                        continue;
                    }

                    full_text.push_str(&text_delta);
                    if let Err(consumer_err) = on_delta(&text_delta) {
                        // Consumer requested stop or failed; cancel and reap worker
                        let cancel_req = HostRequest::Cancel {
                            request_id: self.next_req_id("cancel"),
                            target_request_id: params.request_id.clone(),
                        };
                        let _ = write_host_request(&mut self.stdin, &cancel_req);
                        self.kill_and_reap();
                        return Err(consumer_err);
                    }
                }
                WorkerEvent::GenerationComplete {
                    request_id,
                    finish_reason,
                    usage,
                    tool_proposal,
                    ..
                } => {
                    if request_id != params.request_id {
                        self.kill_and_reap();
                        return Err(LoraError::WorkerCorruptFrame(
                            "GenerationComplete request_id mismatch".to_string(),
                        ));
                    }
                    if cancel_flag.load(Ordering::SeqCst)
                        || finish_reason == FinishReason::Cancelled
                    {
                        return Err(LoraError::Cancelled);
                    }
                    return Ok(WorkerGenerationOutcome {
                        full_text,
                        finish_reason,
                        usage,
                        tool_proposal,
                    });
                }
                WorkerEvent::Error { code, message, .. } => {
                    return Err(Self::map_worker_error(code, message));
                }
                other => {
                    self.kill_and_reap();
                    return Err(LoraError::WorkerCorruptFrame(format!(
                        "unexpected worker event during generation: {other:?}"
                    )));
                }
            }
        }
    }

    fn unload_model(&mut self, model_handle: &str) -> Result<(), LoraError> {
        let request_id = self.next_req_id("unload");
        let req = HostRequest::UnloadModel {
            request_id: request_id.clone(),
            model_handle: model_handle.to_string(),
        };
        if let Err(e) = write_host_request(&mut self.stdin, &req) {
            return Err(self.map_protocol_err(e));
        }

        match read_worker_event(&mut self.stdout) {
            Ok(Some(WorkerEvent::ModelUnloaded {
                request_id: ack_id, ..
            })) if ack_id == request_id => Ok(()),
            Ok(Some(WorkerEvent::Error { code, message, .. })) => {
                Err(Self::map_worker_error(code, message))
            }
            _ => {
                self.kill_and_reap();
                Ok(())
            }
        }
    }

    fn shutdown(&mut self) -> Result<(), LoraError> {
        let request_id = self.next_req_id("shutdown");
        let req = HostRequest::Shutdown { request_id };
        if write_host_request(&mut self.stdin, &req).is_ok() {
            let _ = read_worker_event(&mut self.stdout);
        }
        self.kill_and_reap();
        Ok(())
    }
}

/// Deterministic synthesis logic shared between the standalone `lora-llama-worker` binary
/// and in-process test fixtures (`M1`–`M7`).
pub fn synthesize_deterministic_response(
    prompt: &str,
    tool_calling_enabled: bool,
) -> (Vec<String>, Option<ToolProposalWire>, FinishReason) {
    // If we are in ReadOnlyAssistant mode and no observation has been appended yet,
    // parse the workspace root_id and target file to emit a structured `read_file` proposal.
    if tool_calling_enabled && !prompt.contains("[OBSERVATION_UNTRUSTED_DATA") {
        let root_id = prompt
            .split("WorkspaceRootId=")
            .nth(1)
            .and_then(|s| s.split('.').next())
            .unwrap_or("root-default")
            .trim()
            .to_string();

        let user_part = prompt.split("[USER]:").nth(1).unwrap_or(prompt).trim();

        // Extract requested relative path from `read <path>` or `file:<path>` or default to `guide.md`
        let relative_path = if let Some(rest) = user_part.split("file:").nth(1) {
            rest.split_whitespace()
                .next()
                .unwrap_or("guide.md")
                .trim_matches(|c| c == '"' || c == '\'' || c == '?')
                .to_string()
        } else {
            "guide.md".to_string()
        };

        return (
            vec![],
            Some(ToolProposalWire {
                tool_id: "read_file".to_string(),
                tool_version: 1,
                arguments: ReadFileArgs {
                    root_id,
                    relative_path,
                },
            }),
            FinishReason::ToolCall,
        );
    }

    // If an observation is present, inspect the untrusted data block strictly as data (ignoring any prompt injection!)
    if let Some(obs_section) = prompt.split("[OBSERVATION_UNTRUSTED_DATA ").nth(1) {
        let path = obs_section
            .split("path=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("guide.md");
        let lines = obs_section
            .split("lines=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("L1-L1");
        let body = obs_section
            .split("]:\n")
            .nth(1)
            .and_then(|s| s.split("\n[/OBSERVATION_UNTRUSTED_DATA]").next())
            .unwrap_or("");

        // Look for configuration values (`RECOMMENDED_OPTION=` or `CONFIG_VALUE=`)
        for line in body.lines() {
            if let Some(val) = line
                .strip_prefix("RECOMMENDED_OPTION=")
                .or_else(|| line.strip_prefix("CONFIG_VALUE="))
            {
                let clean_val = val.trim();
                let answer = format!(
                    "Based on the approved file observation, the recommended value is `{clean_val}` (source: [{path}#{lines}])."
                );
                let chunks = answer.split_inclusive(' ').map(|s| s.to_string()).collect();
                return (chunks, None, FinishReason::Stop);
            }
        }

        // If the expected evidence is absent from the observed file, report INSUFFICIENT_EVIDENCE (`M5`)
        let abstain = format!(
            "INSUFFICIENT_EVIDENCE: Checked [{path}#{lines}], but the requested configuration value was not present in the file."
        );
        return (vec![abstain], None, FinishReason::Stop);
    }

    // Standard offline chat response (`M1` / `M4`)
    let reply = format!(
        "Offline local response from pinned llama.cpp worker ({PINNED_LLAMA_CPP_REVISION}): acknowledged prompt."
    );
    let chunks = reply
        .split_inclusive(' ')
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    (chunks, None, FinishReason::Stop)
}
