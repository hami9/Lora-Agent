//! Deterministic offline backend (no inference engine); behaviour is unchanged from the original
//! worker so the acceptance suites keep running against it.

use lora_platform::synthesize_deterministic_response;
use lora_protocol::{write_worker_event, GenerationUsage, WorkerErrorCode, WorkerEvent};
use std::io::Write;
use std::thread;
use std::time::Duration;

use crate::{GenerateArgs, LoadArgs, LoadedModel};

#[derive(Default)]
pub struct DeterministicBackend {
    active: Option<(String, u32)>,
}

impl DeterministicBackend {
    pub fn load(&mut self, args: &LoadArgs) -> Result<LoadedModel, (WorkerErrorCode, String)> {
        let handle = format!("worker-mdl-{}", args.artifact_id);
        self.active = Some((handle.clone(), args.context_length));
        Ok(LoadedModel {
            handle,
            resident_bytes: args.meta.as_ref().map_or(0, |m| m.total_tensor_bytes),
            kv_bytes_per_token: 4096,
        })
    }

    pub fn unload(&mut self) {
        self.active = None;
    }

    /// Returns `Err(())` when the host pipe broke (the worker should exit).
    pub fn generate<W: Write>(
        &mut self,
        writer: &mut W,
        fault_mode: &str,
        g: GenerateArgs,
    ) -> Result<(), ()> {
        let request_id = g.request_id;
        let Some((ref loaded_h, loaded_ctx)) = self.active else {
            return send_err(
                writer,
                request_id,
                WorkerErrorCode::InvalidState,
                "no model loaded in worker",
            );
        };
        if loaded_h != &g.model_handle {
            return send_err(
                writer,
                request_id,
                WorkerErrorCode::InvalidState,
                "stale or unknown model_handle",
            );
        }
        let est_prompt_tokens = ((g.prompt.len() / 4) + 1) as u32;
        if est_prompt_tokens > g.context_length || g.context_length > loaded_ctx {
            return send_err(
                writer,
                request_id,
                WorkerErrorCode::OverContextLimit,
                "prompt exceeds configured context window",
            );
        }

        let (chunks, tool_proposal, finish_reason) =
            synthesize_deterministic_response(&g.prompt, g.tool_calling_enabled);

        let mut seq = 0u64;
        for chunk in &chunks {
            seq += 1;
            if g.simulated_delay_ms > 0 {
                thread::sleep(Duration::from_millis(g.simulated_delay_ms));
            }
            let ev = WorkerEvent::GenerationDelta {
                request_id: request_id.clone(),
                sequence: seq,
                text_delta: chunk.clone(),
                token_count: 1,
            };
            if write_worker_event(writer, &ev).is_err() {
                return Err(());
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
                decode_ms: (chunks.len() as u64).saturating_mul(g.simulated_delay_ms.max(1)),
                peak_rss_bytes: 64 * 1024 * 1024,
            },
            tool_proposal,
        };
        write_worker_event(writer, &done).map_err(|_| ())
    }
}

fn send_err<W: Write>(
    writer: &mut W,
    request_id: String,
    code: WorkerErrorCode,
    message: &str,
) -> Result<(), ()> {
    // The original worker ignored write failures for error replies and kept looping.
    let _ = write_worker_event(
        writer,
        &WorkerEvent::Error {
            request_id: Some(request_id),
            code,
            message: message.to_string(),
            retryable: false,
        },
    );
    Ok(())
}
