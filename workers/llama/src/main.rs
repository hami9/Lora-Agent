//! Isolated native worker executable (`workers/llama`).
//!
//! Exchanges length-prefixed v1 UTF-8 JSON frames over inherited `stdin` and `stdout`.
//! Never opens listening sockets, shell processes, or credentials (`docs/INFERENCE_ENGINE.md`).

// Pieces used only by the native backend (UTF-8 assembly, decode loop, tool parsing) are still
// compiled and unit-tested in default builds.
#![cfg_attr(not(feature = "native-llama-cpp"), allow(dead_code))]

mod backend;
mod cancel;
mod cpucheck;
mod deterministic;
mod generation;
#[cfg(feature = "native-llama-cpp")]
mod native;
mod toolcall;
mod utf8;

use backend::{select_backend, BackendChoice};
use cancel::CancelState;
use deterministic::DeterministicBackend;
use lora_protocol::{
    negotiate_version, parse_gguf_file, read_host_request, sha256_file, validate_nonce,
    write_frame_str, write_worker_event, HostRequest, ProbedModelMetadata, ProtocolError,
    WorkerCapabilities, WorkerErrorCode, WorkerEvent,
};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

/// Parameters of a `LoadModel` request after artifact verification.
pub struct LoadArgs {
    pub artifact_id: String,
    pub artifact_path: String,
    pub context_length: u32,
    pub threads: u32,
    /// `None` when the strict parser could not read the file (native backend only).
    pub meta: Option<ProbedModelMetadata>,
}

pub struct LoadedModel {
    pub handle: String,
    pub resident_bytes: u64,
    pub kv_bytes_per_token: u64,
}

/// Parameters of a `Generate` request.
pub struct GenerateArgs {
    pub request_id: String,
    pub model_handle: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub context_length: u32,
    pub seed: u64,
    pub tool_calling_enabled: bool,
    pub simulated_delay_ms: u64,
}

enum Backend {
    Deterministic(DeterministicBackend),
    #[cfg(feature = "native-llama-cpp")]
    Native(native::NativeBackend),
    /// Native was requested but is not compiled in; every load fails.
    Unavailable,
    /// Native is compiled in but this CPU lacks required features; every load fails.
    #[cfg(feature = "native-llama-cpp")]
    CpuUnsupported(Vec<String>),
}

impl Backend {
    fn new(choice: BackendChoice, _cancel: &Arc<CancelState>) -> Self {
        match choice {
            BackendChoice::Deterministic => Backend::Deterministic(DeterministicBackend::default()),
            #[cfg(feature = "native-llama-cpp")]
            BackendChoice::Native => {
                // Must run before any llama.cpp code: an unsupported CPU would fault.
                let missing = cpucheck::missing_native_cpu_features();
                if missing.is_empty() {
                    Backend::Native(native::NativeBackend::new(Arc::clone(_cancel)))
                } else {
                    Backend::CpuUnsupported(missing)
                }
            }
            #[cfg(not(feature = "native-llama-cpp"))]
            BackendChoice::Native => Backend::Unavailable,
            BackendChoice::NativeUnavailable => Backend::Unavailable,
        }
    }

    /// (engine_name, engine_revision, build_options) reported in `HelloAck`.
    fn identity(&self) -> (String, String, Vec<String>) {
        match self {
            #[cfg(feature = "native-llama-cpp")]
            Backend::Native(_) => (
                "llama.cpp".to_string(),
                lora_protocol::PINNED_LLAMA_CPP_REVISION.to_string(),
                option_env!("LORA_LLAMA_BUILD_OPTIONS")
                    .unwrap_or("cpu")
                    .split(',')
                    .map(str::to_string)
                    .collect(),
            ),
            #[cfg(feature = "native-llama-cpp")]
            Backend::CpuUnsupported(missing) => (
                "llama.cpp".to_string(),
                lora_protocol::PINNED_LLAMA_CPP_REVISION.to_string(),
                vec![format!("cpu-unsupported:missing-{}", missing.join(","))],
            ),
            _ => (
                "lora-deterministic".to_string(),
                "none".to_string(),
                vec!["deterministic".to_string()],
            ),
        }
    }

    fn load(&mut self, args: &LoadArgs) -> Result<LoadedModel, (WorkerErrorCode, String)> {
        match self {
            Backend::Deterministic(b) => b.load(args),
            #[cfg(feature = "native-llama-cpp")]
            Backend::Native(b) => b.load(args),
            #[cfg(feature = "native-llama-cpp")]
            Backend::CpuUnsupported(missing) => Err((
                WorkerErrorCode::UnsupportedCapability,
                format!(
                    "this CPU lacks features required by the native llama.cpp build: {}",
                    missing.join(", ")
                ),
            )),
            Backend::Unavailable => Err((
                WorkerErrorCode::UnsupportedCapability,
                "native llama.cpp backend requested but this worker was built without the                  native-llama-cpp feature"
                    .to_string(),
            )),
        }
    }

    fn accepts_unparsed_metadata(&self) -> bool {
        match self {
            #[cfg(feature = "native-llama-cpp")]
            Backend::Native(_) => true,
            _ => false,
        }
    }

    fn unload(&mut self) {
        match self {
            Backend::Deterministic(b) => b.unload(),
            #[cfg(feature = "native-llama-cpp")]
            Backend::Native(b) => b.unload(),
            Backend::Unavailable => {}
            #[cfg(feature = "native-llama-cpp")]
            Backend::CpuUnsupported(_) => {}
        }
    }

    fn rss_bytes(&self) -> u64 {
        match self {
            #[cfg(feature = "native-llama-cpp")]
            Backend::Native(b) => b.rss_bytes(),
            _ => 32 * 1024 * 1024,
        }
    }

    /// `Err(())` means the host pipe is gone and the worker must exit.
    fn generate<W: Write>(
        &mut self,
        writer: &mut W,
        fault_mode: &str,
        g: GenerateArgs,
    ) -> Result<(), ()> {
        match self {
            Backend::Deterministic(b) => b.generate(writer, fault_mode, g),
            #[cfg(feature = "native-llama-cpp")]
            Backend::Native(b) => b.generate(writer, g),
            #[cfg(feature = "native-llama-cpp")]
            Backend::CpuUnsupported(_) => {
                let _ = write_worker_event(
                    writer,
                    &WorkerEvent::Error {
                        request_id: Some(g.request_id),
                        code: WorkerErrorCode::InvalidState,
                        message: "no model loaded in worker".to_string(),
                        retryable: false,
                    },
                );
                Ok(())
            }
            Backend::Unavailable => {
                let _ = write_worker_event(
                    writer,
                    &WorkerEvent::Error {
                        request_id: Some(g.request_id),
                        code: WorkerErrorCode::InvalidState,
                        message: "no model loaded in worker".to_string(),
                        retryable: false,
                    },
                );
                Ok(())
            }
        }
    }
}

enum Incoming {
    Request(HostRequest),
    Eof,
    Error(ProtocolError),
}

/// Reads frames on a dedicated thread so a `Cancel` can interrupt a generation that is running
/// on the main thread (`intercept` hands matching cancels to the shared abort flag).
fn spawn_reader(cancel: Arc<CancelState>, intercept: bool) -> Receiver<Incoming> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            let msg = match read_host_request(&mut reader) {
                Ok(Some(req)) => {
                    if intercept && cancel.intercept(&req) {
                        continue;
                    }
                    Incoming::Request(req)
                }
                Ok(None) => {
                    cancel.abort();
                    Incoming::Eof
                }
                Err(e) => {
                    cancel.abort();
                    Incoming::Error(e)
                }
            };
            let last = !matches!(msg, Incoming::Request(_));
            if tx.send(msg).is_err() || last {
                break;
            }
        }
    });
    rx
}

/// Digest failures are fatal (outer error); a metadata parse failure is returned separately so
/// the native backend can still fall back to llama.cpp's own reader for files the header parser
/// refuses (e.g. an architecture outside the qualified list).
type VerifiedArtifact = Result<ProbedModelMetadata, String>;

/// Hash the artifact in fixed-size chunks and parse only its header; the file is never loaded
/// into memory as a whole.
fn verify_artifact(
    path: &str,
    expected_sha256: &str,
) -> Result<VerifiedArtifact, (WorkerErrorCode, String)> {
    let path = Path::new(path);
    let actual_sha = sha256_file(path).map_err(|e| {
        (
            WorkerErrorCode::CorruptArtifact,
            format!("worker cannot read artifact file: {e}"),
        )
    })?;
    if !expected_sha256.is_empty() && actual_sha != expected_sha256.to_ascii_lowercase() {
        return Err((
            WorkerErrorCode::CorruptArtifact,
            format!("artifact digest mismatch: expected {expected_sha256}, got {actual_sha}"),
        ));
    }

    Ok(parse_gguf_file(path)
        .map(|p| p.metadata)
        .map_err(|e| e.to_string()))
}

fn verify_and_parse_gguf(
    path: &str,
    expected_sha256: &str,
) -> Result<ProbedModelMetadata, (WorkerErrorCode, String)> {
    verify_artifact(path, expected_sha256)?.map_err(|e| (WorkerErrorCode::CorruptArtifact, e))
}

fn main() {
    let fault_mode = std::env::var("LORA_WORKER_FAULT").unwrap_or_default();

    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    if fault_mode == "crash_on_boot" {
        std::process::exit(139);
    }
    if fault_mode == "corrupt_on_boot" {
        let _ = write_frame_str(&mut writer, "{}");
        return;
    }

    let choice = select_backend(
        std::env::var("LORA_WORKER_BACKEND").ok().as_deref(),
        cfg!(feature = "native-llama-cpp"),
    );
    let cancel = CancelState::new();
    let mut backend = Backend::new(choice, &cancel);
    let incoming = spawn_reader(Arc::clone(&cancel), choice == BackendChoice::Native);

    let mut handshake_completed = false;

    loop {
        let req = match incoming.recv() {
            Ok(Incoming::Request(r)) => r,
            Ok(Incoming::Eof) | Err(_) => break,
            Ok(Incoming::Error(ProtocolError::IncompatibleMajor { .. })) => {
                let _ = write_worker_event(
                    &mut writer,
                    &WorkerEvent::Error {
                        request_id: None,
                        code: WorkerErrorCode::InvalidWireVersion,
                        message: "incompatible wire major version".to_string(),
                        retryable: false,
                    },
                );
                break;
            }
            Ok(Incoming::Error(e)) => {
                let _ = write_worker_event(
                    &mut writer,
                    &WorkerEvent::Error {
                        request_id: None,
                        code: WorkerErrorCode::InvalidState,
                        message: format!("protocol decode error: {e}"),
                        retryable: false,
                    },
                );
                break;
            }
        };

        if !handshake_completed {
            match req {
                HostRequest::Hello { versions, nonce } => {
                    if validate_nonce(&nonce).is_err() {
                        let _ = write_worker_event(
                            &mut writer,
                            &WorkerEvent::Error {
                                request_id: None,
                                code: WorkerErrorCode::InvalidState,
                                message: "invalid 128-bit launch nonce".to_string(),
                                retryable: false,
                            },
                        );
                        break;
                    }
                    match negotiate_version(&versions) {
                        Ok(selected_version) => {
                            handshake_completed = true;
                            let (engine_name, engine_revision, build_options) = backend.identity();
                            let ack = WorkerEvent::HelloAck {
                                selected_version,
                                nonce,
                                engine_name,
                                engine_revision,
                                build_options,
                                capabilities: WorkerCapabilities::default(),
                            };
                            if write_worker_event(&mut writer, &ack).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = write_worker_event(
                                &mut writer,
                                &WorkerEvent::Error {
                                    request_id: None,
                                    code: WorkerErrorCode::InvalidWireVersion,
                                    message: e.to_string(),
                                    retryable: false,
                                },
                            );
                            break;
                        }
                    }
                }
                _ => {
                    // Reject syntactically valid messages sent before Hello handshake (`docs/INFERENCE_ENGINE.md`)
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            request_id: None,
                            code: WorkerErrorCode::InvalidState,
                            message: "received command before Hello handshake".to_string(),
                            retryable: false,
                        },
                    );
                    break;
                }
            }
            continue;
        }

        match req {
            HostRequest::Hello { .. } => {
                let _ = write_worker_event(
                    &mut writer,
                    &WorkerEvent::Error {
                        request_id: None,
                        code: WorkerErrorCode::InvalidState,
                        message: "duplicate Hello message after handshake".to_string(),
                        retryable: false,
                    },
                );
                break;
            }
            HostRequest::ProbeArtifact {
                request_id,
                artifact_path,
                expected_sha256,
                ..
            } => match verify_and_parse_gguf(&artifact_path, &expected_sha256) {
                Ok(metadata) => {
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::ProbeResult {
                            request_id,
                            metadata,
                        },
                    );
                }
                Err((code, message)) => {
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            request_id: Some(request_id),
                            code,
                            message,
                            retryable: false,
                        },
                    );
                }
            },
            HostRequest::LoadModel {
                request_id,
                artifact_id,
                artifact_path,
                expected_sha256,
                context_length,
                threads,
            } => {
                if fault_mode == "crash_on_load" {
                    std::process::exit(139);
                }
                if fault_mode == "corrupt_on_load" {
                    let _ = write_frame_str(&mut writer, "{}");
                    break;
                }
                let verified = verify_artifact(&artifact_path, &expected_sha256).and_then(
                    |parsed| match parsed {
                        Ok(meta) if context_length > meta.context_length => Err((
                            WorkerErrorCode::OverContextLimit,
                            format!(
                                "context {context_length} exceeds model limit {}",
                                meta.context_length
                            ),
                        )),
                        Ok(meta) => Ok(Some(meta)),
                        Err(_) if backend.accepts_unparsed_metadata() => Ok(None),
                        Err(e) => Err((WorkerErrorCode::CorruptArtifact, e)),
                    },
                );
                let loaded = verified.and_then(|meta| {
                    backend.load(&LoadArgs {
                        artifact_id: artifact_id.clone(),
                        artifact_path,
                        context_length,
                        threads,
                        meta,
                    })
                });
                let ev = match loaded {
                    Ok(m) => WorkerEvent::ModelLoaded {
                        request_id,
                        model_handle: m.handle,
                        artifact_id,
                        resident_bytes: m.resident_bytes,
                        kv_bytes_per_token: m.kv_bytes_per_token,
                    },
                    Err((code, message)) => WorkerEvent::Error {
                        request_id: Some(request_id),
                        code,
                        message,
                        retryable: false,
                    },
                };
                let _ = write_worker_event(&mut writer, &ev);
            }
            HostRequest::UnloadModel {
                request_id,
                model_handle,
            } => {
                backend.unload();
                let _ = write_worker_event(
                    &mut writer,
                    &WorkerEvent::ModelUnloaded {
                        request_id,
                        model_handle,
                    },
                );
            }
            HostRequest::Generate {
                request_id,
                model_handle,
                prompt,
                max_tokens,
                context_length,
                seed,
                tool_calling_enabled,
                simulated_delay_ms,
                ..
            } => {
                let g = GenerateArgs {
                    request_id,
                    model_handle,
                    prompt,
                    max_tokens,
                    context_length,
                    seed,
                    tool_calling_enabled,
                    simulated_delay_ms,
                };
                if backend.generate(&mut writer, &fault_mode, g).is_err() {
                    return;
                }
            }
            HostRequest::Cancel { request_id, .. } => {
                // Cancels for an in-flight native generation never reach here (the reader thread
                // consumes them); a stale one is dropped silently so the pipe never desyncs.
                if choice != BackendChoice::Native {
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            request_id: Some(request_id),
                            code: WorkerErrorCode::Cancelled,
                            message: "cancelled".to_string(),
                            retryable: false,
                        },
                    );
                }
            }
            HostRequest::Ping { request_id } => {
                let _ = write_worker_event(
                    &mut writer,
                    &WorkerEvent::Pong {
                        request_id,
                        rss_bytes: backend.rss_bytes(),
                    },
                );
            }
            HostRequest::Shutdown { request_id } => {
                let _ = write_worker_event(&mut writer, &WorkerEvent::ShutdownAck { request_id });
                break;
            }
        }
    }
}
