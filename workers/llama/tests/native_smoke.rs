//! Real-model smoke tests for the native backend.
//!
//! Run with:
//! `LORA_TEST_GGUF=<model.gguf> cargo test -p lora-llama-worker --features native-llama-cpp --release -- --ignored --nocapture`
#![cfg(feature = "native-llama-cpp")]

use lora_protocol::{
    read_worker_event, write_host_request, FinishReason, HostRequest, ProtocolVersion, WorkerEvent,
};
use sha2::{Digest, Sha256};
use std::io::BufWriter;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const NONCE: &str = "afcb26831b679a7621eabc53e375cb3a";

struct Harness {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    events: Receiver<WorkerEvent>,
    handle: String,
    counter: u32,
}

struct GenResult {
    text: String,
    deltas: usize,
    terminal: WorkerEvent,
    first_delta_at: Option<Duration>,
    total: Duration,
}

fn model_path() -> String {
    std::env::var("LORA_TEST_GGUF").expect("set LORA_TEST_GGUF to a GGUF file")
}

impl Harness {
    fn start(context_length: u32) -> Self {
        let path = model_path();
        let mut child = Command::new(env!("CARGO_BIN_EXE_lora-llama-worker"))
            .env_remove("LORA_WORKER_BACKEND")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn worker");
        let mut stdin = BufWriter::new(child.stdin.take().unwrap());
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(ev)) = read_worker_event(&mut stdout) {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });

        write_host_request(
            &mut stdin,
            &HostRequest::Hello {
                versions: vec![ProtocolVersion::V1_0],
                nonce: NONCE.to_string(),
            },
        )
        .unwrap();
        match events.recv_timeout(Duration::from_secs(20)).unwrap() {
            WorkerEvent::HelloAck {
                nonce,
                engine_name,
                engine_revision,
                build_options,
                ..
            } => {
                assert_eq!(nonce, NONCE);
                assert_eq!(engine_name, "llama.cpp");
                assert_eq!(engine_revision, lora_protocol::PINNED_LLAMA_CPP_REVISION);
                println!("HelloAck: {engine_name} {engine_revision} {build_options:?}");
            }
            other => panic!("expected HelloAck, got {other:?}"),
        }

        let bytes = std::fs::read(&path).expect("read model");
        let sha = hex::encode(Sha256::digest(&bytes));
        drop(bytes);
        let started = Instant::now();
        write_host_request(
            &mut stdin,
            &HostRequest::LoadModel {
                request_id: "load-1".into(),
                artifact_id: "smoke".into(),
                artifact_path: path,
                expected_sha256: sha,
                context_length,
                threads: std::env::var("LORA_TEST_THREADS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
        )
        .unwrap();
        let handle = match events.recv_timeout(Duration::from_secs(300)).unwrap() {
            WorkerEvent::ModelLoaded {
                model_handle,
                resident_bytes,
                kv_bytes_per_token,
                ..
            } => {
                println!(
                    "ModelLoaded in {:?}: resident={resident_bytes} kv_bytes_per_token={kv_bytes_per_token}",
                    started.elapsed()
                );
                assert!(resident_bytes > 0 && kv_bytes_per_token > 0);
                model_handle
            }
            other => panic!("LoadModel failed: {other:?}"),
        };
        Self {
            child,
            stdin,
            events,
            handle,
            counter: 0,
        }
    }

    /// Generate; if `cancel_after_first_delta` the worker is sent a Cancel as soon as the first
    /// delta arrives.
    fn generate(
        &mut self,
        prompt: &str,
        max_tokens: u32,
        context_length: u32,
        tools: bool,
        cancel_after_first_delta: bool,
    ) -> GenResult {
        self.counter += 1;
        let id = format!("gen-{}", self.counter);
        let started = Instant::now();
        write_host_request(
            &mut self.stdin,
            &HostRequest::Generate {
                request_id: id.clone(),
                session_id: "s".into(),
                model_handle: self.handle.clone(),
                prompt: prompt.into(),
                max_tokens,
                context_length,
                seed: 42,
                tool_calling_enabled: tools,
                simulated_delay_ms: 0,
            },
        )
        .unwrap();

        let mut text = String::new();
        let mut deltas = 0usize;
        let mut last_seq = 0u64;
        let mut first = None;
        let mut cancelled_sent = false;
        loop {
            let ev = match self.events.recv_timeout(Duration::from_secs(120)) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for worker"),
                Err(RecvTimeoutError::Disconnected) => panic!("worker exited"),
            };
            match ev {
                WorkerEvent::GenerationDelta {
                    request_id,
                    sequence,
                    text_delta,
                    ..
                } => {
                    assert_eq!(request_id, id);
                    assert!(sequence > last_seq, "sequence must increase");
                    last_seq = sequence;
                    deltas += 1;
                    text.push_str(&text_delta);
                    if first.is_none() {
                        first = Some(started.elapsed());
                    }
                    if cancel_after_first_delta && !cancelled_sent {
                        cancelled_sent = true;
                        write_host_request(
                            &mut self.stdin,
                            &HostRequest::Cancel {
                                request_id: "cancel-1".into(),
                                target_request_id: id.clone(),
                            },
                        )
                        .unwrap();
                    }
                }
                done @ WorkerEvent::GenerationComplete { .. } => {
                    return GenResult {
                        text,
                        deltas,
                        terminal: done,
                        first_delta_at: first,
                        total: started.elapsed(),
                    };
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
    }

    fn shutdown(mut self) {
        let _ = write_host_request(
            &mut self.stdin,
            &HostRequest::Shutdown {
                request_id: "bye".into(),
            },
        );
        let _ = self.child.wait();
    }
}

fn finish_of(ev: &WorkerEvent) -> FinishReason {
    match ev {
        WorkerEvent::GenerationComplete { finish_reason, .. } => *finish_reason,
        _ => panic!("not a terminal event"),
    }
}

#[test]
#[ignore = "needs LORA_TEST_GGUF and a native build"]
fn native_answers_cancels_and_proposes_tools() {
    const CTX: u32 = 2048;
    let mut h = Harness::start(CTX);

    // (a) short factual answer
    let r = h.generate(
        "What is 2+2? Answer with one number.",
        64,
        CTX,
        false,
        false,
    );
    let (usage, reason) = match &r.terminal {
        WorkerEvent::GenerationComplete {
            usage,
            finish_reason,
            tool_proposal,
            ..
        } => {
            assert!(tool_proposal.is_none());
            (*usage, *finish_reason)
        }
        _ => unreachable!(),
    };
    let tps = f64::from(usage.completion_tokens) / (usage.decode_ms.max(1) as f64 / 1000.0);
    println!(
        "(a) text={:?} deltas={} finish={reason:?} prompt_tokens={} completion_tokens={} prefill_ms={} decode_ms={} tok/s={tps:.1} rss={}MB first_delta={:?} total={:?}",
        r.text,
        r.deltas,
        usage.prompt_tokens,
        usage.completion_tokens,
        usage.prefill_ms,
        usage.decode_ms,
        usage.peak_rss_bytes / (1024 * 1024),
        r.first_delta_at,
        r.total
    );
    assert!(r.deltas >= 1);
    assert!(matches!(
        reason,
        FinishReason::Stop | FinishReason::MaxTokens
    ));
    assert!(r.text.contains('4'), "expected a 4 in {:?}", r.text);
    assert!(usage.prompt_tokens > 0 && usage.completion_tokens > 0);

    // (b) cancel mid-generation, then a further Generate succeeds
    let started_cancel;
    let r = {
        started_cancel = Instant::now();
        h.generate(
            "Write a very long story about a dragon, at least 2000 words.",
            1500,
            CTX,
            false,
            true,
        )
    };
    let after_first = r.total - r.first_delta_at.unwrap();
    println!(
        "(b) finish={:?} deltas={} first_delta={:?} cancel->terminal latency<={after_first:?} (wall {:?})",
        finish_of(&r.terminal),
        r.deltas,
        r.first_delta_at.unwrap(),
        started_cancel.elapsed()
    );
    assert_eq!(finish_of(&r.terminal), FinishReason::Cancelled);
    assert!(after_first < Duration::from_secs(3), "cancel too slow");

    let r = h.generate("Say hello in one word.", 32, CTX, false, false);
    println!(
        "(b2) after cancel: {:?} finish={:?}",
        r.text,
        finish_of(&r.terminal)
    );
    assert!(r.deltas >= 1);

    // (c) over-context request is rejected cleanly (worker stays alive afterwards)
    h.counter += 1;
    write_host_request(
        &mut h.stdin,
        &HostRequest::Generate {
            request_id: "too-big".into(),
            session_id: "s".into(),
            model_handle: h.handle.clone(),
            prompt: "hi".into(),
            max_tokens: CTX,
            context_length: CTX,
            seed: 1,
            tool_calling_enabled: false,
            simulated_delay_ms: 0,
        },
    )
    .unwrap();
    match h.events.recv_timeout(Duration::from_secs(10)).unwrap() {
        WorkerEvent::Error { code, .. } => {
            assert_eq!(code, lora_protocol::WorkerErrorCode::OverContextLimit)
        }
        other => panic!("expected OverContextLimit, got {other:?}"),
    }

    // (d) tool mode: report what the model did, assert well-formedness if a proposal appears
    let prompt = "[SYSTEM: Mode=ReadOnlyAssistant. WorkspaceRootId=ws1. AllowedTool=read_file(root_id, relative_path).]\n[USER]: Please read the file docs/guide.md in root ws1 and tell me what it recommends.";
    let r = h.generate(prompt, 128, CTX, true, false);
    match &r.terminal {
        WorkerEvent::GenerationComplete {
            finish_reason,
            tool_proposal,
            usage,
            ..
        } => {
            println!(
                "(d) tool mode finish={finish_reason:?} proposal={tool_proposal:?} streamed_text={:?} completion_tokens={}",
                r.text, usage.completion_tokens
            );
            if let Some(p) = tool_proposal {
                assert_eq!(*finish_reason, FinishReason::ToolCall);
                assert_eq!(p.tool_id, "read_file");
                assert_eq!(p.tool_version, 1);
                assert!(!p.arguments.root_id.is_empty() && !p.arguments.relative_path.is_empty());
                assert!(
                    !r.text.contains("<tool_call>"),
                    "markup must not be streamed"
                );
            } else {
                println!("(d) model answered in prose / no valid proposal");
            }
        }
        _ => unreachable!(),
    }

    h.shutdown();
}
