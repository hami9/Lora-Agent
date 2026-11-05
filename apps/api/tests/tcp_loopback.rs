//! End-to-end TCP socket integration test for the `lora-api` loopback server (`docs/API_AND_SDK.md`, `R08`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

fn find_free_loopback_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    format!("127.0.0.1:{}", addr.port())
}

#[test]
fn loopback_api_live_tcp_socket_e2e() {
    let api_bin = env!("CARGO_BIN_EXE_lora-api");
    let tmp = TempDir::new().expect("tempdir");

    // 1. Verify --print-openapi outputs valid OpenAPI 3.1 JSON
    let openapi_out = Command::new(api_bin)
        .arg("--print-openapi")
        .output()
        .expect("run --print-openapi");
    assert!(openapi_out.status.success());
    let openapi_str = String::from_utf8_lossy(&openapi_out.stdout);
    assert!(openapi_str.contains("\"openapi\": \"3.1.0\""));
    assert!(openapi_str.contains("\"/v1/capabilities\""));
    assert!(openapi_str.contains("\"/v1/index\""));
    assert!(openapi_str.contains("\"/v1/search\""));

    // 2. Spawn live `lora-api --once` on loopback port and send authenticated GET /v1/capabilities
    let bind_addr = find_free_loopback_addr();
    let mut child = Command::new(api_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--bind",
            &bind_addr,
            "--token",
            "live-secret-token-42",
            "--once",
        ])
        .spawn()
        .expect("spawn lora-api");

    // Connect with retry while child binds socket
    let mut stream = None;
    for _ in 0..40 {
        if let Ok(s) = TcpStream::connect(&bind_addr) {
            stream = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let mut stream = stream.expect("connect to live lora-api");
    let req = format!(
        "GET /v1/capabilities HTTP/1.1\r\nHost: {bind_addr}\r\nAuthorization: Bearer live-secret-token-42\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();

    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(resp.starts_with("HTTP/1.1 200 OK"));
    assert!(resp.contains("\"api_major\":1"));
    assert!(resp.contains("\"runs.events.sse\""));

    // 3. Import a synthetic GGUF fixture into the store so we can test live execution over TCP
    {
        use lora_core::{
            ApprovalDecision, ApprovalPromptPort, ApprovalRequest, LoraApplicationService,
            LoraError, DEFAULT_SAFETY_RESERVE_BYTES,
        };
        use lora_platform::{
            InProcessWorkerSpawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
        };
        use lora_protocol::{build_synthetic_gguf, SyntheticGgufSpec};
        use std::sync::Arc;

        struct DenyPrompt;
        impl ApprovalPromptPort for DenyPrompt {
            fn prompt_user(&self, _req: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
                Ok(ApprovalDecision::Deny)
            }
        }

        let fixture_path = tmp.path().join("api-fixture.gguf");
        std::fs::write(
            &fixture_path,
            build_synthetic_gguf(&SyntheticGgufSpec::default()),
        )
        .unwrap();
        let app = LoraApplicationService::open(
            tmp.path(),
            DEFAULT_SAFETY_RESERVE_BYTES,
            &OsStoreLockPort,
            Arc::new(SysinfoHardwareProbe),
            Arc::new(OsScopedFileExecutor),
            Arc::new(InProcessWorkerSpawner::default()),
            Arc::new(DenyPrompt),
        )
        .unwrap();
        app.import_model(&fixture_path, Some("api-model"), None)
            .unwrap();
    }

    let ws = tmp.path().join("api_ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("guide.md"),
        "Rollout deployment guidelines\nCONFIG_VALUE=azure_cascade_99\n",
    )
    .unwrap();

    // 4. Multi-request live TCP session: POST /v1/runs (idempotency + 409 conflict + live execute),
    // GET /v1/runs/{id}/events, POST /v1/runs/{id}/cancel, POST /v1/index, POST /v1/search,
    // and live RAG-grounded assistant run with citations
    let bind_addr2 = find_free_loopback_addr();
    let mut child2 = Command::new(api_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--bind",
            &bind_addr2,
            "--token",
            "live-secret-token-42",
            "--max-requests",
            "14",
        ])
        .spawn()
        .expect("spawn multi-request lora-api");

    let send_http = |raw_req: &str| -> String {
        let mut conn = None;
        for _ in 0..40 {
            if let Ok(s) = TcpStream::connect(&bind_addr2) {
                conn = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let mut conn = conn.expect("connect to lora-api");
        conn.write_all(raw_req.as_bytes()).unwrap();
        let mut out = String::new();
        conn.read_to_string(&mut out).unwrap();
        out
    };

    // (a) Create run with Idempotency-Key
    let body_create = r#"{"model":"qwen","prompt":"hello"}"#;
    let r1 = send_http(&format!(
        "POST /v1/runs HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nIdempotency-Key: idem-tcp-1\r\nContent-Length: {}\r\n\r\n{body_create}",
        body_create.len()
    ));
    assert!(r1.starts_with("HTTP/1.1 202 Accepted"));

    // Extract run_id from JSON body
    let body_json_str = r1.split("\r\n\r\n").nth(1).unwrap();
    let created_val: serde_json::Value = serde_json::from_str(body_json_str).unwrap();
    let run_id = created_val["run_id"].as_str().unwrap().to_string();

    // (b) Replay same Idempotency-Key + same body -> returns exact same run_id
    let r2 = send_http(&format!(
        "POST /v1/runs HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nIdempotency-Key: idem-tcp-1\r\nContent-Length: {}\r\n\r\n{body_create}",
        body_create.len()
    ));
    assert!(r2.starts_with("HTTP/1.1 202 Accepted"));
    assert!(r2.contains(&run_id));

    // (c) Reuse same Idempotency-Key with different payload -> 409 Conflict
    let body_diff = r#"{"model":"qwen","prompt":"tampered"}"#;
    let r3 = send_http(&format!(
        "POST /v1/runs HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nIdempotency-Key: idem-tcp-1\r\nContent-Length: {}\r\n\r\n{body_diff}",
        body_diff.len()
    ));
    assert!(r3.starts_with("HTTP/1.1 409 Conflict"));

    // (d) Stream SSE events for the run
    let r4 = send_http(&format!(
        "GET /v1/runs/{run_id}/events HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r4.starts_with("HTTP/1.1 200 OK"));
    assert!(r4.contains("Content-Type: text/event-stream"));
    assert!(r4.contains("event: run_started"));

    // (e) Cancel the run idempotently
    let r5 = send_http(&format!(
        "POST /v1/runs/{run_id}/cancel HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r5.starts_with("HTTP/1.1 200 OK"));
    assert!(r5.contains("\"state\":\"cancelled\""));

    // (f) Cross-origin webpage Origin header rejected with 403 Forbidden (`R08`)
    let r6 = send_http(&format!(
        "GET /v1/models HTTP/1.1\r\nHost: {bind_addr2}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r6.starts_with("HTTP/1.1 403 Forbidden"));

    // (g) Authenticated GET /v1/health and GET /v1/risk-matrix
    let r7 = send_http(&format!(
        "GET /v1/health HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r7.starts_with("HTTP/1.1 200 OK"));
    assert!(r7.contains("\"status\":\"ok\""));

    let r8 = send_http(&format!(
        "GET /v1/risk-matrix HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r8.starts_with("HTTP/1.1 200 OK"));
    assert!(r8.contains("\"risk_id\":\"R18\""));

    // (h) Execute a real chat run via POST /v1/runs with `"execute":true` and verify SSE token_delta + run_completed
    let body_exec = r#"{"model":"api-model","prompt":"Hello from loopback API","execute":true}"#;
    let r9 = send_http(&format!(
        "POST /v1/runs HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nIdempotency-Key: idem-tcp-exec\r\nContent-Length: {}\r\n\r\n{body_exec}",
        body_exec.len()
    ));
    assert!(r9.starts_with("HTTP/1.1 202 Accepted"));
    let exec_json_str = r9.split("\r\n\r\n").nth(1).unwrap();
    let exec_val: serde_json::Value = serde_json::from_str(exec_json_str).unwrap();
    let exec_run_id = exec_val["run_id"].as_str().unwrap();

    let r10 = send_http(&format!(
        "GET /v1/runs/{exec_run_id}/events HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r10.starts_with("HTTP/1.1 200 OK"));
    assert!(r10.contains("event: token_delta"));
    assert!(r10.contains("event: run_completed"));
    assert!(r10.contains("Offline local response"));

    // (i) POST /v1/index indexes workspace files for FTS5 retrieval
    let body_index = serde_json::json!({
        "scope": ws.to_str().unwrap(),
        "files": ["guide.md"],
    })
    .to_string();
    let r11 = send_http(&format!(
        "POST /v1/index HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_index}",
        body_index.len()
    ));
    assert!(r11.starts_with("HTTP/1.1 200 OK"));
    assert!(r11.contains("\"indexed_files\":1"));
    assert!(r11.contains("\"indexed_chunks\":1"));

    // (j) POST /v1/search queries indexed chunks
    let body_search = serde_json::json!({
        "scope": ws.to_str().unwrap(),
        "query": "deployment",
        "limit": 3,
    })
    .to_string();
    let r12 = send_http(&format!(
        "POST /v1/search HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_search}",
        body_search.len()
    ));
    assert!(r12.starts_with("HTTP/1.1 200 OK"));
    assert!(r12.contains("guide.md"));

    // (k) Execute Assistant mode with RAG via POST /v1/runs and verify citation event in SSE
    let body_rag = serde_json::json!({
        "model": "api-model",
        "prompt": "What is the deployment CONFIG_VALUE?",
        "mode": "assistant",
        "scope": ws.to_str().unwrap(),
        "use_rag": true,
        "execute": true,
    })
    .to_string();
    let r13 = send_http(&format!(
        "POST /v1/runs HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nIdempotency-Key: idem-tcp-rag\r\nContent-Length: {}\r\n\r\n{body_rag}",
        body_rag.len()
    ));
    assert!(r13.starts_with("HTTP/1.1 202 Accepted"));
    let rag_json_str = r13.split("\r\n\r\n").nth(1).unwrap();
    let rag_val: serde_json::Value = serde_json::from_str(rag_json_str).unwrap();
    let rag_run_id = rag_val["run_id"].as_str().unwrap();

    let r14 = send_http(&format!(
        "GET /v1/runs/{rag_run_id}/events HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r14.starts_with("HTTP/1.1 200 OK"));
    assert!(r14.contains("event: plan_selected"));
    assert!(r14.contains("event: citation"));
    assert!(r14.contains("event: run_completed"));
    assert!(r14.contains("azure_cascade_99"));

    let status2 = child2.wait().unwrap();
    assert!(status2.success());
}
