//! Opt-in Loopback API composition root (`docs/API_AND_SDK.md`).
//! Enforces strict `127.0.0.1` / `[::1]` bind addresses, DNS-rebinding & Origin checks (`R08`),
//! Authorization Bearer header verification, Idempotency-Key ledger checks, owner-only run state,
//! and SSE `Last-Event-ID` replay windows.

use clap::Parser;
use lora_core::{
    allocate_context_budget, apply_approved_patch, default_risk_register_matrix,
    evaluate_adapter_promotion_gate, now_epoch_ms, prepare_leakage_free_splits,
    preview_checked_patch, rollback_applied_patch, validate_catalog_variant,
    validate_loopback_bind, verify_egress_permit, verify_loopback_http_security, AgentLimits,
    ApiRunManager, ApprovalDecision, ApprovalPromptPort, ApprovalRequest, BrokerSingleUsePermit,
    CatalogVariantDescriptor, CompensatingPatchReceipt, ContextBudgetRequest, EgressConsentPreview,
    EgressPermit, ExecutionMode, ExtensionManifestV1, HeldOutEvaluationReceipt, IdempotencyLedger,
    LoraApplicationService, LoraError, LoraJobConfig, PatchProposal, PrivilegedBrokerGate,
    TrainingExample, TuningCacheKey, TuningProfile, TuningTrialSample,
    DEFAULT_SAFETY_RESERVE_BYTES,
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
    let mut broker_gate = PrivilegedBrokerGate::new(vec![
        "lora-local-service".to_string(),
        "lora-worker".to_string(),
        "lora-daemon".to_string(),
    ]);
    let mut served = 0usize;
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = handle_http_connection(
            &mut stream,
            token,
            &app,
            &mut ledger,
            &mut run_mgr,
            &mut broker_gate,
        );
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
    broker_gate: &mut PrivilegedBrokerGate,
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

    let (path, query_str) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let get_query_param = |key: &str| -> Option<String> {
        query_str.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            if k == key {
                Some(v.to_string())
            } else {
                None
            }
        })
    };

    match (method, path) {
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
        ("GET", "/v1/plugins") => {
            let host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            let body = serde_json::to_string(&host.list()).unwrap_or_else(|_| "[]".to_string());
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("POST", "/v1/plugins/stage") => {
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
            let manifest_val = match req_json.get("manifest") {
                Some(m) => m,
                None => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "manifest field is required",
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
            let manifest: ExtensionManifestV1 = match serde_json::from_value(manifest_val.clone()) {
                Ok(m) => m,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_manifest",
                        "detail": format!("invalid extension manifest: {e}"),
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
            let pkg_sha = req_json
                .get("package_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or("sha256_local");
            let lock_sha = req_json
                .get("lock_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or(pkg_sha);
            let mut host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            match host.stage_and_verify(manifest.clone(), pkg_sha, lock_sha) {
                Ok(state) => {
                    let _ = app.store.save_extension_host(&host);
                    let body = serde_json::json!({
                        "status": "staged",
                        "extension_id": manifest.id,
                        "state": state,
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
        ("POST", p) if p.starts_with("/v1/plugins/") && p.ends_with("/activate") => {
            let id = &p["/v1/plugins/".len()..p.len() - "/activate".len()];
            let req_json: serde_json::Value = serde_json::from_str(&body_str).unwrap_or_default();
            let grants: Vec<String> = req_json
                .get("grants")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let mut host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            match host.activate(id, &grants) {
                Ok(state) => {
                    let _ = app.store.save_extension_host(&host);
                    let body = serde_json::json!({
                        "status": "activated",
                        "extension_id": id,
                        "state": state,
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
        ("POST", p) if p.starts_with("/v1/plugins/") && p.ends_with("/disable") => {
            let id = &p["/v1/plugins/".len()..p.len() - "/disable".len()];
            let mut host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            match host.disable_and_revoke(id) {
                Ok(state) => {
                    let _ = app.store.save_extension_host(&host);
                    let body = serde_json::json!({
                        "status": "disabled",
                        "extension_id": id,
                        "state": state,
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
        ("POST", p) if p.starts_with("/v1/plugins/") && p.ends_with("/fault") => {
            let id = &p["/v1/plugins/".len()..p.len() - "/fault".len()];
            let mut host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            match host.record_fault(id) {
                Ok(state) => {
                    let _ = app.store.save_extension_host(&host);
                    let body = serde_json::json!({
                        "status": "fault_recorded",
                        "extension_id": id,
                        "state": state,
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
        ("POST", p) if p.starts_with("/v1/plugins/") && p.ends_with("/rollback") => {
            let id = &p["/v1/plugins/".len()..p.len() - "/rollback".len()];
            let mut host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            match host.rollback(id) {
                Ok(prev) => {
                    let _ = app.store.save_extension_host(&host);
                    let body = serde_json::json!({
                        "status": "rolled_back",
                        "extension_id": id,
                        "previous_digest": prev,
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
        ("GET", p) if p.starts_with("/v1/plugins/") => {
            let id = &p["/v1/plugins/".len()..];
            let host = match app.store.load_extension_host() {
                Ok(h) => h,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": e.reason_code(),
                        "detail": e.redacted_message(),
                    })
                    .to_string();
                    return write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    );
                }
            };
            match host.get(id) {
                Some(rec) => {
                    let body = serde_json::to_string(rec).unwrap_or_default();
                    write_http_response(stream, "200 OK", "application/json", &body)
                }
                None => {
                    let err = serde_json::json!({
                        "code": "not_found",
                        "detail": format!("extension '{id}' not found"),
                    })
                    .to_string();
                    write_http_response(stream, "404 Not Found", "application/json", &err)
                }
            }
        }
        ("POST", "/v1/adapter/evaluate") => {
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
            let base_sha = req_json
                .get("base_model_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or("sha256:base");
            let tok_sha = req_json
                .get("tokenizer_template_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or("sha256:tok");
            let installed_sha = req_json
                .get("installed_base_sha256")
                .and_then(|v| v.as_str())
                .unwrap_or(base_sha);
            let job = LoraJobConfig {
                job_id: req_json
                    .get("job_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("job-eval-1")
                    .to_string(),
                base_model_sha256: base_sha.to_string(),
                tokenizer_template_sha256: tok_sha.to_string(),
                dataset_sha256: req_json
                    .get("dataset_sha256")
                    .and_then(|v| v.as_str())
                    .unwrap_or("dataset-sha")
                    .to_string(),
                rank: req_json.get("rank").and_then(|v| v.as_u64()).unwrap_or(8) as u32,
                alpha: req_json.get("alpha").and_then(|v| v.as_u64()).unwrap_or(16) as u32,
                target_modules: vec!["q_proj".to_string(), "v_proj".to_string()],
                epochs: 3,
                context_length: 2048,
                microbatch_size: 4,
                seed: 42,
            };
            let eval = HeldOutEvaluationReceipt {
                base_task_score_pct: req_json
                    .get("base_task_score_pct")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(70) as u32,
                adapter_task_score_pct: req_json
                    .get("adapter_task_score_pct")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(85) as u32,
                general_regression_pct: req_json
                    .get("general_regression_pct")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                schema_valid_pct: req_json
                    .get("schema_valid_pct")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100) as u32,
                target_runtime_load_verified: req_json
                    .get("target_runtime_load_verified")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            };
            match evaluate_adapter_promotion_gate(&job, installed_sha, eval) {
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
                    write_http_response(stream, "400 Bad Request", "application/json", &err)
                }
            }
        }
        ("POST", "/v1/adapter/dataset-split") => {
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
            let license = req_json
                .get("license")
                .and_then(|v| v.as_str())
                .unwrap_or("Apache-2.0");
            let examples: Vec<TrainingExample> = match req_json
                .get("examples")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                Some(ex) => ex,
                None => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "examples array of TrainingExample is required",
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
            match prepare_leakage_free_splits(&examples, license) {
                Ok(split) => {
                    let body = serde_json::to_string(&split).unwrap_or_default();
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
        ("POST", "/v1/hardware/tune") => {
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
            let key = if let Some(key_val) = req_json.get("key") {
                match serde_json::from_value::<TuningCacheKey>(key_val.clone()) {
                    Ok(k) => k,
                    Err(e) => {
                        let err = serde_json::json!({
                            "code": "malformed_request",
                            "detail": format!("invalid key object: {e}"),
                        })
                        .to_string();
                        return write_http_response(
                            stream,
                            "400 Bad Request",
                            "application/json",
                            &err,
                        );
                    }
                }
            } else {
                let model_alias = req_json
                    .get("model_alias")
                    .and_then(|v| v.as_str())
                    .unwrap_or("api-model");
                let model_sha = match app.registry.resolve_ready_model(model_alias) {
                    Ok((manifest, _)) => manifest
                        .files
                        .first()
                        .map(|f| f.sha256.clone())
                        .unwrap_or_default(),
                    Err(_) => "sha256:model-default".to_string(),
                };
                let snap = app
                    .hw_probe
                    .capture_snapshot(app.data_dir())
                    .unwrap_or_else(|_| lora_core::HostSnapshot {
                        os_name: std::env::consts::OS.to_string(),
                        cpu_arch: std::env::consts::ARCH.to_string(),
                        cpu_brand: "Generic-CPU".to_string(),
                        physical_cores: 4,
                        logical_cores: 8,
                        cpu_isa_features: vec!["x86_64".to_string()],
                        total_ram_bytes: 16 * 1024 * 1024 * 1024,
                        available_ram_bytes: 8 * 1024 * 1024 * 1024,
                        free_disk_bytes: 100 * 1024 * 1024 * 1024,
                        charging_state: lora_core::ChargingState::AcOnline,
                        sample_timestamp_ms: now_epoch_ms(),
                        unavailable_sensors: vec![],
                    });
                let ctx = req_json
                    .get("context_length")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2048) as u32;
                let prof = match req_json
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .unwrap_or("automatic")
                {
                    "low_memory" | "low-memory" => TuningProfile::LowMemory,
                    "battery_saver" | "battery-saver" => TuningProfile::BatterySaver,
                    "max_throughput" | "max-throughput" | "maximum_performance" => {
                        TuningProfile::MaximumPerformance
                    }
                    _ => TuningProfile::Automatic,
                };
                TuningCacheKey {
                    hardware_fingerprint: snap.cpu_brand,
                    os_arch: format!("{}-{}", snap.os_name, snap.cpu_arch),
                    worker_revision: "lora-worker-v1".to_string(),
                    model_sha256: model_sha,
                    context_length: ctx,
                    profile: prof,
                }
            };
            let base_threads = req_json
                .get("baseline_threads")
                .and_then(|v| v.as_u64())
                .unwrap_or(4) as u32;
            let base_batch = req_json
                .get("baseline_batch_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(256) as u32;
            let base_tps = req_json
                .get("baseline_tok_per_sec_x100")
                .and_then(|v| v.as_u64())
                .unwrap_or(2400) as u32;
            let ttl = req_json
                .get("ttl_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(86_400_000);
            let trials: Vec<TuningTrialSample> = req_json
                .get("trials")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(|| {
                    vec![
                        TuningTrialSample {
                            threads: base_threads,
                            batch_size: base_batch,
                            tok_per_sec_x100: base_tps,
                            variance_pct: 2,
                            peak_rss_bytes: 64 * 1024 * 1024,
                            thermal_or_memory_warning: false,
                        },
                        TuningTrialSample {
                            threads: base_threads.saturating_add(2).min(16),
                            batch_size: base_batch.saturating_mul(2).min(1024),
                            tok_per_sec_x100: base_tps.saturating_mul(120) / 100,
                            variance_pct: 3,
                            peak_rss_bytes: 72 * 1024 * 1024,
                            thermal_or_memory_warning: false,
                        },
                    ]
                });
            match app.store.evaluate_and_store_tuning_receipt(
                key,
                base_threads,
                base_batch,
                base_tps,
                &trials,
                ttl,
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
                    write_http_response(stream, "400 Bad Request", "application/json", &err)
                }
            }
        }
        ("GET", "/v1/hardware/tuning-receipts") => match app.store.list_tuning_receipts() {
            Ok(receipts) => {
                let body = serde_json::to_string(&receipts).unwrap_or_default();
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
        },
        ("GET", "/v1/audit/events") => {
            let run_id = get_query_param("run_id");
            match app.store.list_audit_events(run_id.as_deref()) {
                Ok(events) => {
                    let body = serde_json::to_string(&events).unwrap_or_default();
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
        ("POST", "/v1/audit/prune") => {
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
            let keep_latest = req_json
                .get("keep_latest")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;
            match app.store.prune_audit_retention(keep_latest) {
                Ok(pruned) => {
                    let body = serde_json::json!({
                        "status": "pruned",
                        "pruned_records": pruned,
                        "keep_latest": keep_latest,
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
                    write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        &err,
                    )
                }
            }
        }
        ("GET", "/v1/audit/export") => {
            let run_id = get_query_param("run_id");
            match app.store.export_redacted_audit_bundle(run_id.as_deref()) {
                Ok(bundle) => write_http_response(stream, "200 OK", "application/json", &bundle),
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
        ("POST", "/v1/egress/preview") => {
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
            let preview: EgressConsentPreview = match serde_json::from_value(req_json) {
                Ok(p) => p,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid egress preview payload: {e}"),
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
            let digest = preview.compute_preview_digest();
            let body = serde_json::json!({
                "preview": preview,
                "preview_digest": digest,
            })
            .to_string();
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("POST", "/v1/egress/verify") => {
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
            let permit: EgressPermit = match req_json
                .get("permit")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                Some(p) => p,
                None => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "permit object is required",
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
            let actual: EgressConsentPreview = match req_json
                .get("actual_payload")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                Some(a) => a,
                None => {
                    let err = serde_json::json!({
                        "code": "missing_parameter",
                        "detail": "actual_payload object is required",
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
            match verify_egress_permit(&permit, &actual, now_epoch_ms()) {
                Ok(()) => {
                    let body = serde_json::json!({
                        "status": "verified",
                        "permit_id": permit.permit_id,
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
        ("POST", "/v1/catalog/validate") => {
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

            let descriptor: CatalogVariantDescriptor = if req_json.get("descriptor").is_some() {
                match serde_json::from_value(req_json["descriptor"].clone()) {
                    Ok(d) => d,
                    Err(e) => {
                        let err = serde_json::json!({
                            "code": "malformed_request",
                            "detail": format!("invalid descriptor: {e}"),
                        })
                        .to_string();
                        return write_http_response(
                            stream,
                            "400 Bad Request",
                            "application/json",
                            &err,
                        );
                    }
                }
            } else {
                match serde_json::from_value(req_json.clone()) {
                    Ok(d) => d,
                    Err(e) => {
                        let err = serde_json::json!({
                            "code": "malformed_request",
                            "detail": format!("invalid descriptor: {e}"),
                        })
                        .to_string();
                        return write_http_response(
                            stream,
                            "400 Bad Request",
                            "application/json",
                            &err,
                        );
                    }
                }
            };

            let redirect_hops = req_json
                .get("redirect_hops")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8;

            match validate_catalog_variant(&descriptor, redirect_hops) {
                Ok(()) => {
                    let body = serde_json::json!({
                        "status": "valid",
                        "model_id": descriptor.model_id,
                        "filename": descriptor.filename,
                        "size_bytes": descriptor.size_bytes,
                        "sha256_verified": true,
                        "strong_etag": descriptor.strong_etag,
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
        ("POST", "/v1/context-budget") => {
            let req: ContextBudgetRequest = match serde_json::from_str(&body_str) {
                Ok(r) => r,
                Err(e) => {
                    let err = serde_json::json!({
                        "code": "malformed_request",
                        "detail": format!("invalid context budget request: {e}"),
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

            match allocate_context_budget(&req) {
                Ok(alloc) => {
                    let body = serde_json::to_string(&alloc).unwrap_or_default();
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
        ("POST", "/v1/broker/validate") => {
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

            let permit: BrokerSingleUsePermit =
                match serde_json::from_value(if req_json.get("permit").is_some() {
                    req_json["permit"].clone()
                } else {
                    req_json.clone()
                }) {
                    Ok(p) => p,
                    Err(e) => {
                        let err = serde_json::json!({
                            "code": "malformed_request",
                            "detail": format!("invalid broker permit: {e}"),
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

            if let Some(arr) = req_json
                .get("allowlisted_services")
                .and_then(|v| v.as_array())
            {
                for s in arr.iter().filter_map(|v| v.as_str()) {
                    broker_gate.allow_service(s);
                }
            }

            let now = now_epoch_ms();
            match broker_gate.validate_and_consume_permit(&permit, now) {
                Ok(status) => {
                    let body = serde_json::json!({
                        "status": status,
                        "permit_id": permit.permit_id,
                        "valid": true,
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
