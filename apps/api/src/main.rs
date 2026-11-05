//! Opt-in Loopback API composition root (`docs/API_AND_SDK.md`).
//! Enforces strict `127.0.0.1` / `[::1]` bind addresses, DNS-rebinding & Origin checks (`R08`),
//! Authorization Bearer header verification, Idempotency-Key ledger checks, owner-only run state,
//! and SSE `Last-Event-ID` replay windows.

use clap::Parser;
use lora_core::{
    apply_approved_patch, default_risk_register_matrix, now_epoch_ms, preview_checked_patch,
    rollback_applied_patch, validate_loopback_bind, verify_loopback_http_security, AgentLimits,
    ApiRunManager, ApprovalDecision, ApprovalPromptPort, ApprovalRequest, CompensatingPatchReceipt,
    ExecutionMode, IdempotencyLedger, LoraApplicationService, LoraError, PatchProposal,
    TuningProfile, DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_platform::{
    resolve_default_worker_spawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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

    /// Path to isolated `lora-llama-worker` binary (defaults to sibling executable or in-process shim)
    #[arg(long, env = "LORA_WORKER_BIN")]
    worker_bin: Option<PathBuf>,

    /// Worker backend selection (`native` or `deterministic`)
    #[arg(long, env = "LORA_WORKER_BACKEND")]
    worker_backend: Option<String>,

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

    /// Maximum number of requests to serve before exiting (0 = unlimited)
    #[arg(long, default_value_t = 0)]
    max_requests: usize,

    /// Automatically approve file read requests within verified workspace scope (for headless test/automation)
    #[arg(long)]
    auto_approve: bool,
}

struct ConfigurableApiPrompt {
    auto_approve: bool,
}

impl ApprovalPromptPort for ConfigurableApiPrompt {
    fn prompt_user(&self, _request: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
        if self.auto_approve {
            Ok(ApprovalDecision::ApproveOnce)
        } else {
            Ok(ApprovalDecision::Deny)
        }
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
    let worker_spawner =
        resolve_default_worker_spawner(cli.worker_bin, cli.worker_backend.as_deref());
    let approval_ui = Arc::new(ConfigurableApiPrompt {
        auto_approve: cli.auto_approve,
    });

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
    let mut served = 0usize;
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = handle_http_connection(&mut stream, token, &app, &mut ledger, &mut run_mgr);
        served += 1;
        if cli.once || (cli.max_requests > 0 && served >= cli.max_requests) {
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
        ("GET", p) if p.starts_with("/v1/models/") => {
            let alias = &p["/v1/models/".len()..];
            match app.registry.resolve_ready_model(alias) {
                Ok((manifest, _blob_path)) => {
                    let body = serde_json::to_string(&manifest).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(stream, "404 Not Found", "application/json", &err)
                }
            }
        }
        ("DELETE", p) if p.starts_with("/v1/models/") => {
            let alias = &p["/v1/models/".len()..];
            match app.delete_model(alias) {
                Ok(outcome) => {
                    let body = serde_json::to_string(&outcome).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(stream, "404 Not Found", "application/json", &err)
                }
            }
        }
        ("GET", "/v1/risk-matrix") => {
            let matrix = default_risk_register_matrix();
            let body = serde_json::to_string(&matrix).unwrap_or_else(|_| "[]".to_string());
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("POST", "/v1/index") => {
            let req_json: serde_json::Value = match serde_json::from_str(&body_str) {
                Ok(j) => j,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid json body: {e}"),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_str = match req_json.get("scope").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s,
                _ => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "missing or empty 'scope' field",
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let files: Vec<String> = match req_json.get("files").and_then(|v| v.as_array()) {
                Some(arr) if !arr.is_empty() => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                _ => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "missing or empty 'files' array",
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_path = PathBuf::from(scope_str);
            let root = match app.file_executor.register_root(&scope_path) {
                Ok(r) => r,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            app.policy.register_scoped_root(root.clone());
            let max_bytes = req_json
                .get("max_file_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(32 * 1024) as usize;
            match app.store.index_scoped_files(
                &root,
                &files,
                max_bytes,
                app.file_executor.as_ref(),
                &|| Ok(()),
            ) {
                Ok(receipt) => {
                    let body = serde_json::to_string(&receipt).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    )
                }
            }
        }
        ("POST", "/v1/search") => {
            let req_json: serde_json::Value = match serde_json::from_str(&body_str) {
                Ok(j) => j,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid json body: {e}"),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_str = match req_json.get("scope").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s,
                _ => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "missing or empty 'scope' field",
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let query = match req_json.get("query").and_then(|v| v.as_str()) {
                Some(q) if !q.trim().is_empty() => q,
                _ => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "missing or empty 'query' field",
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_path = PathBuf::from(scope_str);
            let root = match app.file_executor.register_root(&scope_path) {
                Ok(r) => r,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            app.policy.register_scoped_root(root.clone());
            let limit = req_json.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            match app
                .store
                .search_lexical_fts(&root.root_id, query, limit, &app.policy)
            {
                Ok(hits) => {
                    let body = serde_json::to_string(&hits).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    )
                }
            }
        }
        ("POST", "/v1/patch/preview") => {
            let req_json: serde_json::Value = match serde_json::from_str(&body_str) {
                Ok(j) => j,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid json body: {e}"),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_str = req_json.get("scope").and_then(|v| v.as_str()).unwrap_or("");
            let file = req_json.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let expected_sha256 = req_json
                .get("expected_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let old_snippet = req_json
                .get("old_snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_snippet = req_json
                .get("new_snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if scope_str.is_empty() || file.is_empty() || old_snippet.is_empty() {
                let err = serde_json::json!({
                    "code": "missing_parameter",
                    "detail": "scope, file, and old_snippet are required",
                })
                .to_string();
                return write_http_response(stream, "400 Bad Request", "application/json", &err);
            }
            let scope_path = PathBuf::from(scope_str);
            let root = match app.file_executor.register_root(&scope_path) {
                Ok(r) => r,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let proposal = PatchProposal {
                root_id: root.root_id.clone(),
                relative_path: file.to_string(),
                expected_before_sha256: expected_sha256.to_string(),
                old_snippet: old_snippet.to_string(),
                new_snippet: new_snippet.to_string(),
            };
            match preview_checked_patch(&root, &proposal) {
                Ok(prev) => {
                    let body = serde_json::to_string(&prev).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(stream, "400 Bad Request", "application/json", &err)
                }
            }
        }
        ("POST", "/v1/patch/apply") => {
            let req_json: serde_json::Value = match serde_json::from_str(&body_str) {
                Ok(j) => j,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid json body: {e}"),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_str = req_json.get("scope").and_then(|v| v.as_str()).unwrap_or("");
            let file = req_json.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let expected_sha256 = req_json
                .get("expected_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let old_snippet = req_json
                .get("old_snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_snippet = req_json
                .get("new_snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let approval_digest = req_json
                .get("approval_digest")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if scope_str.is_empty() || file.is_empty() || approval_digest.is_empty() {
                let err = serde_json::json!({
                    "code": "missing_parameter",
                    "detail": "scope, file, and approval_digest are required",
                })
                .to_string();
                return write_http_response(stream, "400 Bad Request", "application/json", &err);
            }
            let scope_path = PathBuf::from(scope_str);
            let root = match app.file_executor.register_root(&scope_path) {
                Ok(r) => r,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let proposal = PatchProposal {
                root_id: root.root_id.clone(),
                relative_path: file.to_string(),
                expected_before_sha256: expected_sha256.to_string(),
                old_snippet: old_snippet.to_string(),
                new_snippet: new_snippet.to_string(),
            };
            match apply_approved_patch(&root, &proposal, approval_digest) {
                Ok(rcpt) => {
                    let body = serde_json::to_string(&rcpt).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(stream, "400 Bad Request", "application/json", &err)
                }
            }
        }
        ("POST", "/v1/patch/rollback") => {
            let req_json: serde_json::Value = match serde_json::from_str(&body_str) {
                Ok(j) => j,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid json body: {e}"),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            let scope_str = req_json.get("scope").and_then(|v| v.as_str()).unwrap_or("");
            let rcpt_val = req_json.get("receipt");
            if scope_str.is_empty() || rcpt_val.is_none() {
                let err = serde_json::json!({
                    "code": "missing_parameter",
                    "detail": "scope and receipt are required",
                })
                .to_string();
                return write_http_response(stream, "400 Bad Request", "application/json", &err);
            }
            let rcpt: CompensatingPatchReceipt =
                match serde_json::from_value(rcpt_val.unwrap().clone()) {
                    Ok(r) => r,
                    Err(e) => {
                        let err = serde_json::json!({
                            "code": "malformed_request",
                            "detail": format!("invalid receipt payload: {e}"),
                        })
                        .to_string();
                        return write_http_response(
                            stream,
                            "400 Bad Request",
                            "application/json",
                            &err,
                        );
                    }
                };
            let scope_path = PathBuf::from(scope_str);
            let root = match app.file_executor.register_root(&scope_path) {
                Ok(r) => r,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &err,
                    );
                }
            };
            match rollback_applied_patch(&root, &rcpt) {
                Ok(()) => {
                    let body = serde_json::json!({
                        "status": "rolled_back",
                        "relative_path": rcpt.relative_path,
                        "restored_sha256": rcpt.before_sha256,
                    })
                    .to_string();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    write_http_response(stream, "400 Bad Request", "application/json", &err)
                }
            }
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

                if let Ok(req_json) = serde_json::from_str::<serde_json::Value>(&body_str) {
                    let execute = req_json
                        .get("execute")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if execute {
                        let model = req_json
                            .get("model")
                            .and_then(|v| v.as_str())
                            .unwrap_or("default");
                        let prompt = req_json
                            .get("prompt")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Hello");
                        let context = req_json
                            .get("context")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(2048) as u32;
                        let mode_str = req_json
                            .get("mode")
                            .and_then(|v| v.as_str())
                            .unwrap_or("chat");
                        let use_rag = req_json
                            .get("use_rag")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let scope_path = req_json
                            .get("scope")
                            .and_then(|v| v.as_str())
                            .map(PathBuf::from);

                        let (mode, workspace_opt) = match mode_str {
                            "assistant" | "read_only_assistant" => {
                                (ExecutionMode::ReadOnlyAssistant, scope_path.as_deref())
                            }
                            _ => (ExecutionMode::ChatOnly, None),
                        };

                        let mut limits = AgentLimits::default();
                        if let Some(lim_obj) = req_json.get("limits") {
                            if let Some(mtc) =
                                lim_obj.get("max_tool_calls").and_then(|v| v.as_u64())
                            {
                                limits.max_tool_calls = mtc as u32;
                            }
                            if let Some(mfb) =
                                lim_obj.get("max_file_bytes").and_then(|v| v.as_u64())
                            {
                                limits.max_file_bytes = mfb as usize;
                            }
                            if let Some(dl) =
                                lim_obj.get("run_deadline_secs").and_then(|v| v.as_u64())
                            {
                                limits.run_deadline_secs = dl;
                            }
                            if let Some(mot) =
                                lim_obj.get("max_output_tokens").and_then(|v| v.as_u64())
                            {
                                limits.max_output_tokens = mot as u32;
                            }
                        }

                        let cancel = Arc::new(AtomicBool::new(false));
                        let mut deltas = Vec::new();
                        match app.run_rag_task(
                            &resolved_run_id,
                            principal,
                            model,
                            context,
                            TuningProfile::Automatic,
                            mode,
                            workspace_opt,
                            prompt,
                            &limits,
                            use_rag,
                            &cancel,
                            &mut |delta| {
                                deltas.push(delta.to_string());
                                Ok(())
                            },
                        ) {
                            Ok((plan, outcome)) => {
                                let plan_data = serde_json::json!({
                                    "explanation": plan.explanation,
                                    "weights_bytes": plan.estimate.weights_bytes,
                                    "kv_cache_bytes": plan.estimate.kv_cache_bytes,
                                })
                                .to_string();
                                let _ = run_mgr.append_event(
                                    &resolved_run_id,
                                    "plan_selected",
                                    &plan_data,
                                );

                                for chunk in deltas {
                                    let ev_data =
                                        serde_json::json!({ "text_delta": chunk }).to_string();
                                    let _ = run_mgr.append_event(
                                        &resolved_run_id,
                                        "token_delta",
                                        &ev_data,
                                    );
                                }
                                for cit in &outcome.citations {
                                    let cit_data = serde_json::to_string(cit).unwrap_or_default();
                                    let _ = run_mgr.append_event(
                                        &resolved_run_id,
                                        "citation",
                                        &cit_data,
                                    );
                                }
                                let done_data = serde_json::json!({
                                    "status": "succeeded",
                                    "answer": outcome.answer,
                                    "turns_used": outcome.turns_used,
                                    "citations": outcome.citations,
                                })
                                .to_string();
                                let _ = run_mgr.append_event(
                                    &resolved_run_id,
                                    "run_completed",
                                    &done_data,
                                );
                            }
                            Err(err) => {
                                let err_data = serde_json::json!({
                                    "status": "failed",
                                    "code": err.reason_code(),
                                    "detail": err.redacted_message(),
                                })
                                .to_string();
                                let _ =
                                    run_mgr.append_event(&resolved_run_id, "run_failed", &err_data);
                            }
                        }
                    }
                }
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
