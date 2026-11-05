//! Opt-in Loopback API composition root (`docs/API_AND_SDK.md`).
//! Enforces strict `127.0.0.1` / `[::1]` bind addresses, DNS-rebinding & Origin checks (`R08`),
//! Authorization Bearer header verification, Idempotency-Key ledger checks, owner-only run state,
//! and SSE `Last-Event-ID` replay windows.

use clap::Parser;
use lora_core::{
    now_epoch_ms, validate_loopback_bind, verify_loopback_http_security, ApiRunManager,
    ApprovalDecision, ApprovalPromptPort, ApprovalRequest, IdempotencyLedger,
    LoraApplicationService, LoraError, DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_platform::{
    InProcessWorkerSpawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(
    name = "lora-api",
    version,
    about = "Lora Agent opt-in loopback API server (127.0.0.1 / [::1] only)"
)]
struct ApiCli {
    /// Per-user data directory
    #[arg(long, env = "LORA_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Numeric loopback socket address (`127.0.0.1:8421` or `[::1]:8421`)
    #[arg(long, default_value = "127.0.0.1:8421")]
    bind: String,

    /// Required per-session Bearer token
    #[arg(long, env = "LORA_API_TOKEN")]
    token: Option<String>,

    /// Print the OpenAPI 3.1 contract and exit
    #[arg(long)]
    print_openapi: bool,

    /// Process a single request and exit (used for deterministic qualification tests)
    #[arg(long)]
    once: bool,
}

struct DenyByDefaultApiPrompt;
impl ApprovalPromptPort for DenyByDefaultApiPrompt {
    fn prompt_user(&self, _request: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
        Ok(ApprovalDecision::Deny)
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(appdata).join("LoraAgent");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/share/lora-agent");
    }
    std::env::temp_dir().join("lora-agent-data")
}

fn main() {
    let cli = ApiCli::parse();
    if cli.print_openapi {
        print!("{}", include_str!("../openapi.json"));
        return;
    }

    if let Err(e) = validate_loopback_bind(&cli.bind) {
        eprintln!("error [{:?}]: {}", e.reason_code(), e);
        std::process::exit(1);
    }

    let Some(token) = cli.token.as_deref().filter(|t| !t.trim().is_empty()) else {
        eprintln!(
            "error [policy_denied]: --token or LORA_API_TOKEN is required to start loopback API"
        );
        std::process::exit(1);
    };

    let data_dir = cli.data_dir.unwrap_or_else(default_data_dir);
    let lock_port = OsStoreLockPort;
    let hw_probe = Arc::new(SysinfoHardwareProbe);
    let file_executor = Arc::new(OsScopedFileExecutor);
    let worker_spawner = Arc::new(InProcessWorkerSpawner::default());
    let approval_ui = Arc::new(DenyByDefaultApiPrompt);

    let app = match LoraApplicationService::open(
        &data_dir,
        DEFAULT_SAFETY_RESERVE_BYTES,
        &lock_port,
        hw_probe,
        file_executor,
        worker_spawner,
        approval_ui,
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error [{:?}]: {}", e.reason_code(), e);
            std::process::exit(1);
        }
    };

    let listener = match TcpListener::bind(&cli.bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error [io_error]: failed to bind {}: {e}", cli.bind);
            std::process::exit(1);
        }
    };

    let mut ledger = IdempotencyLedger::new();
    let mut run_mgr = ApiRunManager::default();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = handle_http_connection(&mut stream, token, &app, &mut ledger, &mut run_mgr);
        if cli.once {
            break;
        }
    }
}

fn write_http_response(
    stream: &mut std::net::TcpStream,
    status_line: &str,
    content_type: &str,
    body: &str,
) -> Result<(), LoraError> {
    let resp = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

fn handle_http_connection(
    stream: &mut std::net::TcpStream,
    expected_token: &str,
    app: &LoraApplicationService,
    ledger: &mut IdempotencyLedger,
    run_mgr: &mut ApiRunManager,
) -> Result<(), LoraError> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("GET");
    let path_and_query = parts.get(1).copied().unwrap_or("/");

    let mut host = None;
    let mut origin = None;
    let mut auth = None;
    let mut cookie = None;
    let mut idempotency_key = None;
    let mut last_event_id = None;
    let mut content_length: usize = 0;

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            match key.as_str() {
                "host" => host = Some(val),
                "origin" => origin = Some(val),
                "authorization" => auth = Some(val),
                "cookie" => cookie = Some(val),
                "idempotency-key" => idempotency_key = Some(val),
                "last-event-id" => last_event_id = Some(val),
                "content-length" => content_length = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    if content_length > 1024 * 1024 {
        let body = r#"{"code":"payload_too_large","retryable":false,"detail":"request exceeds 1 MiB limit"}"#;
        return write_http_response(stream, "413 Payload Too Large", "application/json", body);
    }

    let mut body_bytes = vec![0u8; content_length];
    if !body_bytes.is_empty() {
        reader.read_exact(&mut body_bytes)?;
    }
    let body_str = String::from_utf8_lossy(&body_bytes);

    if let Err(e) = verify_loopback_http_security(
        host.as_deref(),
        origin.as_deref(),
        auth.as_deref(),
        cookie.as_deref(),
        path_and_query,
        expected_token,
    ) {
        let body = serde_json::json!({
            "code": e.reason_code(),
            "retryable": false,
            "detail": e.redacted_message(),
        })
        .to_string();
        return write_http_response(stream, "403 Forbidden", "application/json", &body);
    }

    let principal = "loopback-api-client";

    match (method, path_and_query) {
        ("GET", "/v1/health") => {
            let body = r#"{"status":"ok","service":"lora-api","api_version":1}"#;
            write_http_response(stream, "200 OK", "application/json", body)
        }
        ("GET", "/v1/capabilities") => {
            let caps = run_mgr.capabilities_for(principal);
            let body = serde_json::to_string(&caps).unwrap_or_else(|_| "{}".to_string());
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("GET", "/v1/models") => {
            let models = app.registry.list_models().unwrap_or_default();
            let body = serde_json::to_string(&models).unwrap_or_else(|_| "[]".to_string());
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("POST", "/v1/runs") => {
            let candidate_run_id = format!("run-api-{}", now_epoch_ms());
            let resolved_run_id = if let Some(idem) = idempotency_key.as_deref() {
                match ledger.register_or_verify(principal, idem, &body_str, &candidate_run_id) {
                    Ok(rid) => rid,
                    Err(e) => {
                        let body = serde_json::json!({
                            "code": e.reason_code(),
                            "retryable": false,
                            "detail": e.redacted_message(),
                        })
                        .to_string();
                        return write_http_response(
                            stream,
                            "409 Conflict",
                            "application/json",
                            &body,
                        );
                    }
                }
            } else {
                candidate_run_id
            };

            if run_mgr.get_run(&resolved_run_id, principal).is_err() {
                run_mgr.create_run(&resolved_run_id, principal);
                let _ = run_mgr.append_event(
                    &resolved_run_id,
                    "run_started",
                    r#"{"status":"running"}"#,
                );
            }

            let body = serde_json::json!({
                "status": "accepted",
                "run_id": resolved_run_id,
                "events_url": format!("/v1/runs/{resolved_run_id}/events"),
            })
            .to_string();
            write_http_response(stream, "202 Accepted", "application/json", &body)
        }
        _ if path_and_query.starts_with("/v1/runs/") => {
            let rest = &path_and_query["/v1/runs/".len()..];
            if let Some(run_id) = rest.strip_suffix("/cancel") {
                if method != "POST" {
                    return write_http_response(
                        stream,
                        "405 Method Not Allowed",
                        "application/json",
                        r#"{"code":"method_not_allowed"}"#,
                    );
                }
                match run_mgr.cancel_run(run_id, principal) {
                    Ok(summary) => {
                        let body = serde_json::to_string(&summary).unwrap_or_default();
                        write_http_response(stream, "200 OK", "application/json", &body)
                    }
                    Err(e) => {
                        let body = serde_json::json!({
                            "code": e.reason_code(),
                            "detail": e.redacted_message(),
                        })
                        .to_string();
                        write_http_response(stream, "404 Not Found", "application/json", &body)
                    }
                }
            } else if let Some(run_id) = rest.strip_suffix("/events") {
                match run_mgr.replay_events(run_id, principal, last_event_id.as_deref()) {
                    Ok(replay) => {
                        let mut sse_out = String::new();
                        if replay.replay_gap_detected {
                            sse_out.push_str(&format!(
                                "event: replay_gap\ndata: {{\"run_id\":\"{run_id}\",\"current_state\":\"{}\"}}\n\n",
                                replay.current_state
                            ));
                        } else {
                            for ev in &replay.events {
                                sse_out.push_str(&ev.to_sse_wire());
                            }
                        }
                        write_http_response(stream, "200 OK", "text/event-stream", &sse_out)
                    }
                    Err(e) => {
                        let body = serde_json::json!({
                            "code": e.reason_code(),
                            "detail": e.redacted_message(),
                        })
                        .to_string();
                        write_http_response(stream, "404 Not Found", "application/json", &body)
                    }
                }
            } else {
                match run_mgr.get_run(rest, principal) {
                    Ok(summary) => {
                        let body = serde_json::to_string(&summary).unwrap_or_default();
                        write_http_response(stream, "200 OK", "application/json", &body)
                    }
                    Err(e) => {
                        let body = serde_json::json!({
                            "code": e.reason_code(),
                            "detail": e.redacted_message(),
                        })
                        .to_string();
                        write_http_response(stream, "404 Not Found", "application/json", &body)
                    }
                }
            }
        }
        _ => {
            let body = r#"{"code":"not_found","retryable":false,"detail":"unknown endpoint"}"#;
            write_http_response(stream, "404 Not Found", "application/json", body)
        }
    }
}
