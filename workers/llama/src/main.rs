//! Isolated native worker executable (`workers/llama`).
//!
//! Exchanges length-prefixed v1 UTF-8 JSON frames over inherited `stdin` and `stdout`.
//! Never opens listening sockets, shell processes, or credentials (`docs/INFERENCE_ENGINE.md`).

use lora_platform::synthesize_deterministic_response;
use lora_protocol::{
    negotiate_version, parse_gguf_bytes, read_host_request, validate_nonce, write_frame_str,
    write_worker_event, GenerationUsage, HostRequest, ProtocolError, WorkerCapabilities,
    WorkerErrorCode, WorkerEvent, PINNED_LLAMA_CPP_REVISION,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, BufReader, BufWriter};
use std::path::Path;
use std::thread;
use std::time::Duration;

fn verify_and_parse_gguf(
    path: &str,
    expected_sha256: &str,
) -> Result<lora_protocol::ProbedModelMetadata, (WorkerErrorCode, String)> {
    let bytes = fs::read(Path::new(path)).map_err(|e| {
        (
            WorkerErrorCode::CorruptArtifact,
            format!("worker cannot read artifact file: {e}"),
        )
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual_sha = hex::encode(hasher.finalize());
    if !expected_sha256.is_empty() && actual_sha != expected_sha256.to_ascii_lowercase() {
        return Err((
            WorkerErrorCode::CorruptArtifact,
            format!("artifact digest mismatch: expected {expected_sha256}, got {actual_sha}"),
        ));
    }

    let parsed =
        parse_gguf_bytes(&bytes).map_err(|e| (WorkerErrorCode::CorruptArtifact, e.to_string()))?;
    Ok(parsed.metadata)
}

fn main() {
    let fault_mode = std::env::var("LORA_WORKER_FAULT").unwrap_or_default();

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());

    if fault_mode == "crash_on_boot" {
        std::process::exit(139);
    }
    if fault_mode == "corrupt_on_boot" {
        let _ = write_frame_str(&mut writer, "{}");
        return;
    }

    let mut handshake_completed = false;
    let mut active_handle: Option<(String, u32)> = None;

    loop {
        let req = match read_host_request(&mut reader) {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(ProtocolError::IncompatibleMajor { .. }) => {
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
            Err(e) => {
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
                            let ack = WorkerEvent::HelloAck {
                                selected_version,
                                nonce,
                                engine_name: "llama.cpp".to_string(),
                                engine_revision: PINNED_LLAMA_CPP_REVISION.to_string(),
                                build_options: vec![
                                    "cpu".to_string(),
                                    "avx2".to_string(),
                                    "fma".to_string(),
                                ],
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
                ..
            } => {
                if fault_mode == "crash_on_load" {
                    std::process::exit(139);
                }
                if fault_mode == "corrupt_on_load" {
                    let _ = write_frame_str(&mut writer, "{}");
                    break;
                }
                match verify_and_parse_gguf(&artifact_path, &expected_sha256) {
                    Ok(meta) => {
                        if context_length > meta.context_length {
                            let _ = write_worker_event(
                                &mut writer,
                                &WorkerEvent::Error {
                                    request_id: Some(request_id),
                                    code: WorkerErrorCode::OverContextLimit,
                                    message: format!(
                                        "context {context_length} exceeds model limit {}",
                                        meta.context_length
                                    ),
                                    retryable: false,
                                },
                            );
                            continue;
                        }
                        let handle = format!("worker-mdl-{artifact_id}");
                        active_handle = Some((handle.clone(), context_length));
                        let _ = write_worker_event(
                            &mut writer,
                            &WorkerEvent::ModelLoaded {
                                request_id,
                                model_handle: handle,
                                artifact_id,
                                resident_bytes: meta.total_tensor_bytes,
                                kv_bytes_per_token: 4096,
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
                }
            }
            HostRequest::UnloadModel {
                request_id,
                model_handle,
            } => {
                active_handle = None;
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
                context_length,
                tool_calling_enabled,
                simulated_delay_ms,
                ..
            } => {
                let Some((ref loaded_h, loaded_ctx)) = active_handle else {
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            request_id: Some(request_id),
                            code: WorkerErrorCode::InvalidState,
                            message: "no model loaded in worker".to_string(),
                            retryable: false,
                        },
                    );
                    continue;
                };
                if loaded_h != &model_handle {
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            request_id: Some(request_id),
                            code: WorkerErrorCode::InvalidState,
                            message: "stale or unknown model_handle".to_string(),
                            retryable: false,
                        },
                    );
                    continue;
                }
                let est_prompt_tokens = ((prompt.len() / 4) + 1) as u32;
                if est_prompt_tokens > context_length || context_length > loaded_ctx {
                    let _ = write_worker_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            request_id: Some(request_id),
                            code: WorkerErrorCode::OverContextLimit,
                            message: "prompt exceeds configured context window".to_string(),
                            retryable: false,
                        },
                    );
                    continue;
                }

                let (chunks, tool_proposal, finish_reason) =
                    synthesize_deterministic_response(&prompt, tool_calling_enabled);

                let mut seq = 0u64;
                for chunk in &chunks {
                    seq += 1;
                    if simulated_delay_ms > 0 {
                        thread::sleep(Duration::from_millis(simulated_delay_ms));
                    }
                    let ev = WorkerEvent::GenerationDelta {
                        request_id: request_id.clone(),
                        sequence: seq,
                        text_delta: chunk.clone(),
                        token_count: 1,
                    };
                    if write_worker_event(&mut writer, &ev).is_err() {
                        return;
                    }
                    if fault_mode == "crash_mid_decode" && seq == 1 {
                        std::process::exit(139);
                    }
                }

                seq += 1;
                let done = WorkerEvent::GenerationComplete {
                    request_id,
                    sequence: seq,
                    finish_reason,
                    usage: GenerationUsage {
                        prompt_tokens: est_prompt_tokens,
                        completion_tokens: chunks.len() as u32,
                        prefill_ms: 4,
                        decode_ms: (chunks.len() as u64).saturating_mul(simulated_delay_ms.max(1)),
                        peak_rss_bytes: 64 * 1024 * 1024,
                    },
                    tool_proposal,
                };
                if write_worker_event(&mut writer, &done).is_err() {
                    return;
                }
            }
            HostRequest::Cancel { request_id, .. } => {
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
            HostRequest::Ping { request_id } => {
                let _ = write_worker_event(
                    &mut writer,
                    &WorkerEvent::Pong {
                        request_id,
                        rss_bytes: 32 * 1024 * 1024,
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
