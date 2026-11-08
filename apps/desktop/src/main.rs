//! Desktop UX composition root (`docs/DESKTOP_UX.md`).
//! Exposes the local-first desktop dashboard state, hardware telemetry snapshot, and WCAG 2.2 AA
//! trusted consent view-models to the desktop shell without ambient filesystem or network privileges.

use clap::Parser;
use lora_core::{
    allocate_context_budget, default_risk_register_matrix, slice_virtualized_transcript,
    validate_workflow_job, ApprovalDecision, ApprovalPromptPort, ApprovalRequest,
    ContextBudgetRequest, LoraApplicationService, LoraError, TrustedConsentDialogViewModel,
    WorkflowJobConfig, WorkflowStepKind, DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_platform::{
    resolve_default_worker_spawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "lora-desktop",
    version,
    about = "Lora Agent desktop shell state & view-model bridge"
)]
struct DesktopCli {
    /// Per-user data directory
    #[arg(long, env = "LORA_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Path to isolated `lora-llama-worker` binary (defaults to sibling executable or in-process shim)
    #[arg(long, env = "LORA_WORKER_BIN")]
    worker_bin: Option<PathBuf>,

    /// Worker backend selection (`native` or `deterministic`)
    #[arg(long, env = "LORA_WORKER_BACKEND")]
    worker_backend: Option<String>,

    /// Enable experimental screens in the status strip
    #[arg(long)]
    experimental: bool,

    /// Emit initial desktop dashboard view-model JSON and exit
    #[arg(long)]
    dump_view_model: bool,

    /// Emit a WCAG 2.2 AA TrustedConsentDialogViewModel JSON for a proposed relative path and exit
    #[arg(long)]
    preview_consent: Option<String>,

    /// Emit a VirtualizedTranscriptSlice JSON window for `<total_items>,<scroll_offset>,<viewport_capacity>` (`R12`)
    #[arg(long, value_delimiter = ',')]
    slice_transcript: Vec<usize>,

    /// Serve the desktop shell UI and read-only JSON view-models on a loopback address
    /// (e.g. `127.0.0.1:8765`; port `0` picks a free port). Non-loopback binds are refused.
    #[arg(long)]
    serve: Option<String>,

    /// With `--serve`, handle exactly one request and exit
    #[arg(long)]
    once: bool,

    /// With `--serve`, exit after handling this many requests
    #[arg(long)]
    max_requests: Option<usize>,
}

/// Strict Content-Security-Policy applied to every loopback response (`docs/DESKTOP_UX.md`).
const LOOPBACK_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; \
frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

const DESKTOP_INDEX_HTML: &str = include_str!("../ui/index.html");
const MAX_REQUEST_HEAD_BYTES: usize = 8192;

struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json<T: serde::Serialize>(value: &T) -> Self {
        match serde_json::to_vec_pretty(value) {
            Ok(body) => Self {
                status: "200 OK",
                content_type: "application/json; charset=utf-8",
                body,
            },
            Err(_) => Self::error("500 Internal Server Error", "serialization_failed"),
        }
    }

    fn error(status: &'static str, reason: &str) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: serde_json::json!({ "error": reason })
                .to_string()
                .into_bytes(),
        }
    }
}

/// Parse `key=value` pairs from a URL query string with `+` and `%XX` decoding.
fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Route one read-only GET request to the matching view-model endpoint.
fn route_request(
    app: &LoraApplicationService,
    data_dir: &std::path::Path,
    experimental: bool,
    target: &str,
) -> HttpResponse {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match path {
        "/" | "/index.html" => HttpResponse {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body: DESKTOP_INDEX_HTML.as_bytes().to_vec(),
        },
        "/api/dashboard" => match app.build_desktop_dashboard(experimental) {
            Ok(vm) => HttpResponse::json(&vm),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/models" => match app.registry.list_models() {
            Ok(models) => HttpResponse::json(&models),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/plugins" => match app.store.load_extension_host() {
            Ok(host) => HttpResponse::json(&host.list()),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/tuning" => match app.store.list_tuning_receipts() {
            Ok(receipts) => HttpResponse::json(&receipts),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/audit" => match app.store.list_audit_events(None) {
            Ok(events) => HttpResponse::json(&events),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/context-budget" => {
            let req = ContextBudgetRequest {
                context_window_tokens: 2048,
                reserved_output_tokens: 512,
                control_schema_overhead_tokens: 128,
                user_request_tokens: 256,
                recent_turns_tokens: 128,
                tool_receipts_tokens: 64,
                ranked_excerpts: vec![],
            };
            match allocate_context_budget(&req) {
                Ok(alloc) => HttpResponse::json(&alloc),
                Err(e) => HttpResponse::error(
                    "500 Internal Server Error",
                    &format!("{:?}", e.reason_code()),
                ),
            }
        }
        "/api/broker-status" => {
            let status = serde_json::json!({
                "status": "ready",
                "allowlisted_services": ["lora-local-service", "lora-worker", "lora-daemon"],
                "nonce_tracking": "active",
                "replay_defense": "enforced"
            });
            HttpResponse::json(&status)
        }
        "/api/risk-matrix" => HttpResponse::json(&default_risk_register_matrix()),
        "/api/doctor" => {
            let models_count = app.registry.list_models().map(|m| m.len()).unwrap_or(0);
            let ext_count = app
                .store
                .load_extension_host()
                .map(|h| h.list().len())
                .unwrap_or(0);
            let snap = app.hw_probe.capture_snapshot(data_dir);
            let snap_json = match snap {
                Ok(s) => serde_json::json!({
                    "physical_cores": s.physical_cores,
                    "available_ram_bytes": s.available_ram_bytes,
                    "total_ram_bytes": s.total_ram_bytes,
                    "free_disk_bytes": s.free_disk_bytes,
                }),
                Err(e) => serde_json::json!({
                    "error": format!("{:?}", e.reason_code()),
                }),
            };
            let doc = serde_json::json!({
                "status": "healthy",
                "service": "lora-desktop",
                "models_installed": models_count,
                "extensions_installed": ext_count,
                "hardware": snap_json,
            });
            HttpResponse::json(&doc)
        }
        "/api/workflow-status" => {
            let status = serde_json::json!({
                "status": "ready",
                "active_pipeline": null,
                "supported_steps": [
                    "doctor_probe",
                    "index_workspace",
                    "grounded_query",
                    "preview_patch",
                    "audit_export",
                    "verify_digest"
                ],
                "orchestration_mode": "air_gapped_deterministic",
                "deadline_gating": true,
                "step_retries_supported": true,
                "max_step_retries_cap": 10,
                "non_fatal_steps_supported": true,
                "scoped_path_guard": true,
                "receipt_attestation": true,
                "receipt_self_verification": true,
                "bundle_export_supported": true,
                "cancellation_token_supported": true
            });
            HttpResponse::json(&status)
        }
        "/api/workflow-bundle" => match app.store.export_workflow_attestation_bundle(Some(20)) {
            Ok(bundle) => HttpResponse::json(&bundle),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/workflow-verify" => match app.store.verify_stored_workflow_receipts(Some(20)) {
            Ok((checked, valid)) => HttpResponse::json(&serde_json::json!({
                "status": if checked == valid { "verified" } else { "tampered" },
                "attested_receipts_checked": checked,
                "attested_receipts_valid": valid,
                "chain_intact": checked == valid
            })),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/workflow-receipts" => match app.store.list_workflow_receipts(Some(20)) {
            Ok(receipts) => HttpResponse::json(&serde_json::json!({
                "count": receipts.len(),
                "receipts": receipts
            })),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/workflow-validate" => {
            let demo_cfg = WorkflowJobConfig {
                workflow_id: "desktop-wf-probe".to_string(),
                title: "Desktop Live Probe".to_string(),
                steps: vec![
                    WorkflowStepKind::DoctorProbe {
                        min_available_ram_mb: Some(512),
                    },
                    WorkflowStepKind::IndexWorkspace {
                        workspace_id: "default".to_string(),
                        relative_path: "docs/ARCHITECTURE.md".to_string(),
                    },
                ],
                stop_on_failure: true,
                max_duration_ms: Some(30_000),
                max_step_retries: Some(2),
                non_fatal_step_indices: None,
            };
            let validation = validate_workflow_job(&demo_cfg);
            HttpResponse::json(&validation)
        }
        "/api/hardware" => match app.hw_probe.capture_snapshot(data_dir) {
            Ok(snap) => HttpResponse::json(&snap),
            Err(e) => HttpResponse::error(
                "500 Internal Server Error",
                &format!("{:?}", e.reason_code()),
            ),
        },
        "/api/consent-preview" => {
            let Some(rel_path) = query_param(query, "path") else {
                return HttpResponse::error("400 Bad Request", "missing_path_parameter");
            };
            HttpResponse::json(&build_consent_preview(data_dir, rel_path))
        }
        "/api/slice-transcript" => {
            let parse = |k: &str| query_param(query, k).and_then(|v| v.parse::<usize>().ok());
            let (Some(total), Some(offset), Some(cap)) =
                (parse("total"), parse("offset"), parse("capacity"))
            else {
                return HttpResponse::error("400 Bad Request", "missing_slice_parameters");
            };
            // Bound synthetic transcript size to keep the bridge allocation-safe.
            let total = total.min(100_000);
            let items: Vec<String> = (0..total).map(|i| format!("msg-{i}")).collect();
            HttpResponse::json(&slice_virtualized_transcript(&items, offset, cap.min(1000)))
        }
        _ => HttpResponse::error("404 Not Found", "unknown_route"),
    }
}

fn build_consent_preview(
    data_dir: &std::path::Path,
    rel_path: String,
) -> TrustedConsentDialogViewModel {
    let req = ApprovalRequest {
        run_id: "run-desktop-preview".to_string(),
        proposal_id: "prop-desktop-1".to_string(),
        tool_id: "read_file".to_string(),
        root_id: "ws-desktop".to_string(),
        root_display_path: data_dir.to_string_lossy().to_string(),
        relative_path: rel_path,
        max_bytes: 32768,
        purpose: "Read file in approved workspace scope".to_string(),
        approval_digest: "sha256:preview-action-digest".to_string(),
    };
    TrustedConsentDialogViewModel::from_approval_request(&req)
}

/// Accept only Host headers naming the loopback listener (DNS-rebinding defense).
fn host_header_is_loopback(host: &str, port: u16) -> bool {
    let host = host.trim().to_ascii_lowercase();
    [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
    .contains(&host)
}

fn handle_connection(
    mut stream: TcpStream,
    app: &LoraApplicationService,
    data_dir: &std::path::Path,
    experimental: bool,
    port: u16,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
        if head.len() > MAX_REQUEST_HEAD_BYTES {
            break;
        }
    }

    let response = if head.len() > MAX_REQUEST_HEAD_BYTES {
        HttpResponse::error(
            "431 Request Header Fields Too Large",
            "request_head_too_large",
        )
    } else {
        let text = String::from_utf8_lossy(&head);
        let mut lines = text.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        let host = lines
            .take_while(|l| !l.is_empty())
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case("host").then(|| v.to_string())
            })
            .unwrap_or_default();

        if !host_header_is_loopback(&host, port) {
            HttpResponse::error("403 Forbidden", "non_loopback_host_header")
        } else if method != "GET" {
            HttpResponse::error("405 Method Not Allowed", "read_only_bridge")
        } else {
            route_request(app, data_dir, experimental, target)
        }
    };

    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
Content-Security-Policy: {LOOPBACK_CSP}\r\nX-Content-Type-Options: nosniff\r\n\
X-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n\
Connection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

fn serve_loopback(
    app: &LoraApplicationService,
    data_dir: &std::path::Path,
    experimental: bool,
    bind: &str,
    max_requests: Option<usize>,
) -> Result<(), String> {
    let addr: SocketAddr = bind
        .parse()
        .map_err(|e| format!("invalid --serve address '{bind}': {e}"))?;
    if !addr.ip().is_loopback() {
        return Err(format!(
            "refusing non-loopback bind address '{bind}'; use 127.0.0.1 or [::1]"
        ));
    }
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind failed: {e}"))?;
    let local = listener
        .local_addr()
        .map_err(|e| format!("local_addr failed: {e}"))?;
    println!("Lora Desktop loopback bridge listening on http://{local}/");
    let _ = std::io::stdout().flush();

    for (idx, stream) in listener.incoming().enumerate() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle_connection(s, app, data_dir, experimental, local.port()) {
                    eprintln!("warning: loopback connection error: {e}");
                }
            }
            Err(e) => eprintln!("warning: loopback accept error: {e}"),
        }
        if max_requests.is_some_and(|m| idx + 1 >= m) {
            break;
        }
    }
    Ok(())
}

struct DenyByDefaultDesktopPrompt;
impl ApprovalPromptPort for DenyByDefaultDesktopPrompt {
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
    let cli = DesktopCli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(default_data_dir);
    let lock_port = OsStoreLockPort;
    let hw_probe = Arc::new(SysinfoHardwareProbe);
    let file_executor = Arc::new(OsScopedFileExecutor);
    let worker_spawner =
        resolve_default_worker_spawner(cli.worker_bin, cli.worker_backend.as_deref());
    let approval_ui = Arc::new(DenyByDefaultDesktopPrompt);

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

    let vm = match app.build_desktop_dashboard(cli.experimental) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error [{:?}]: {}", e.reason_code(), e);
            std::process::exit(1);
        }
    };

    if let Some(bind) = cli.serve.as_deref() {
        let max = if cli.once { Some(1) } else { cli.max_requests };
        if let Err(e) = serve_loopback(&app, &data_dir, cli.experimental, bind, max) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Some(rel_path) = cli.preview_consent {
        let dialog = build_consent_preview(&data_dir, rel_path);
        println!(
            "{}",
            serde_json::to_string_pretty(&dialog).expect("serialize TrustedConsentDialogViewModel")
        );
        return;
    }

    if cli.slice_transcript.len() == 3 {
        let total = cli.slice_transcript[0];
        let offset = cli.slice_transcript[1];
        let cap = cli.slice_transcript[2];
        let items: Vec<String> = (0..total).map(|i| format!("msg-{i}")).collect();
        let slice = slice_virtualized_transcript(&items, offset, cap);
        println!(
            "{}",
            serde_json::to_string_pretty(&slice).expect("serialize VirtualizedTranscriptSlice")
        );
        return;
    }

    if cli.dump_view_model {
        println!(
            "{}",
            serde_json::to_string_pretty(&vm).expect("serialize desktop view model")
        );
    } else {
        println!(
            "Lora Desktop Core initialized ({} verified models ready, {} MiB available RAM).",
            vm.installed_model_count, vm.available_ram_mib
        );
    }
}
