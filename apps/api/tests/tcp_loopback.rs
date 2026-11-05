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
    assert!(openapi_str.contains("\"/v1/patch/preview\""));
    assert!(openapi_str.contains("\"/v1/patch/apply\""));
    assert!(openapi_str.contains("\"/v1/patch/rollback\""));

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
            "37",
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

    // (l) GET /v1/models/{alias} returns full model manifest
    let r15 = send_http(&format!(
        "GET /v1/models/api-model HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r15.starts_with("HTTP/1.1 200 OK"));
    assert!(r15.contains("\"display_alias\":\"api-model\""));

    // (m) POST /v1/patch/preview computes diff and approval digest
    let guide_sha = lora_protocol::sha256_file(&ws.join("guide.md")).unwrap();
    let body_prev = serde_json::json!({
        "scope": ws.to_str().unwrap(),
        "file": "guide.md",
        "expected_sha256": guide_sha,
        "old_snippet": "CONFIG_VALUE=azure_cascade_99",
        "new_snippet": "CONFIG_VALUE=azure_cascade_100",
    })
    .to_string();
    let r16 = send_http(&format!(
        "POST /v1/patch/preview HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_prev}",
        body_prev.len()
    ));
    assert!(r16.starts_with("HTTP/1.1 200 OK"));
    let prev_val: serde_json::Value =
        serde_json::from_str(r16.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let approval_digest = prev_val["approval_digest"].as_str().unwrap().to_string();

    // (n) POST /v1/patch/apply modifies file and returns CompensatingPatchReceipt
    let body_apply = serde_json::json!({
        "scope": ws.to_str().unwrap(),
        "file": "guide.md",
        "expected_sha256": guide_sha,
        "old_snippet": "CONFIG_VALUE=azure_cascade_99",
        "new_snippet": "CONFIG_VALUE=azure_cascade_100",
        "approval_digest": approval_digest,
    })
    .to_string();
    let r17 = send_http(&format!(
        "POST /v1/patch/apply HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_apply}",
        body_apply.len()
    ));
    assert!(r17.starts_with("HTTP/1.1 200 OK"));
    let rcpt_json = r17.split("\r\n\r\n").nth(1).unwrap();
    let rcpt_val: serde_json::Value = serde_json::from_str(rcpt_json).unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.join("guide.md")).unwrap(),
        "Rollout deployment guidelines\nCONFIG_VALUE=azure_cascade_100\n"
    );

    // (o) POST /v1/patch/rollback reverts the file safely
    let body_rollback = serde_json::json!({
        "scope": ws.to_str().unwrap(),
        "receipt": rcpt_val,
    })
    .to_string();
    let r18 = send_http(&format!(
        "POST /v1/patch/rollback HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_rollback}",
        body_rollback.len()
    ));
    assert!(r18.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(
        std::fs::read_to_string(ws.join("guide.md")).unwrap(),
        "Rollout deployment guidelines\nCONFIG_VALUE=azure_cascade_99\n"
    );

    // (p) DELETE /v1/models/{alias} retires model and collects unreferenced blob
    let r19 = send_http(&format!(
        "DELETE /v1/models/api-model HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r19.starts_with("HTTP/1.1 200 OK"));
    assert!(r19.contains("\"final_state\":\"collected\""));
    assert!(r19.contains("\"blob_garbage_collected\":true"));

    // (q) GET retired model now returns 404
    let r20 = send_http(&format!(
        "GET /v1/models/api-model HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r20.starts_with("HTTP/1.1 404 Not Found"));

    // (r) POST /v1/plugins/stage stages candidate extension manifest
    let body_plugin_stage = serde_json::json!({
        "manifest": {
            "manifest_version": 1,
            "id": "org.lora.wordcount",
            "version": "1.0.0",
            "kind": "wasm-component",
            "host_api": { "major": 1, "minimum_minor": 0 },
            "entry": "component.wasm",
            "exports": ["tool.wordcount.v1"],
            "requests": ["input.text"],
            "dependencies": []
        },
        "package_sha256": "sha256_wordcount_v1",
        "lock_sha256": "sha256_wordcount_v1"
    })
    .to_string();
    let r21 = send_http(&format!(
        "POST /v1/plugins/stage HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_plugin_stage}",
        body_plugin_stage.len()
    ));
    assert!(r21.starts_with("HTTP/1.1 200 OK"));
    assert!(r21.contains("\"status\":\"staged\""));
    assert!(r21.contains("\"extension_id\":\"org.lora.wordcount\""));
    assert!(r21.contains("\"state\":\"disabled\""));

    // (s) POST /v1/plugins/{id}/activate grants approved capabilities and enables extension
    let body_act = serde_json::json!({
        "grants": ["input.text"]
    })
    .to_string();
    let r22 = send_http(&format!(
        "POST /v1/plugins/org.lora.wordcount/activate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_act}",
        body_act.len()
    ));
    assert!(r22.starts_with("HTTP/1.1 200 OK"));
    assert!(r22.contains("\"status\":\"activated\""));
    assert!(r22.contains("\"state\":\"active\""));

    // (t) GET /v1/plugins lists installed extensions
    let r23 = send_http(&format!(
        "GET /v1/plugins HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r23.starts_with("HTTP/1.1 200 OK"));
    assert!(r23.contains("\"id\":\"org.lora.wordcount\""));
    assert!(r23.contains("\"approved_capabilities\":[\"input.text\"]"));

    // (u) GET /v1/plugins/{id} returns extension record
    let r24 = send_http(&format!(
        "GET /v1/plugins/org.lora.wordcount HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r24.starts_with("HTTP/1.1 200 OK"));
    assert!(r24.contains("\"state\":\"active\""));

    // (v) POST /v1/plugins/{id}/disable revokes capabilities
    let r25 = send_http(&format!(
        "POST /v1/plugins/org.lora.wordcount/disable HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: 2\r\n\r\n{{}}"
    ));
    assert!(r25.starts_with("HTTP/1.1 200 OK"));
    assert!(r25.contains("\"status\":\"disabled\""));
    assert!(r25.contains("\"state\":\"disabled\""));

    // (w) POST /v1/plugins/stage updates to v1.1.0 retaining previous compatible digest
    let body_plugin_update = serde_json::json!({
        "manifest": {
            "manifest_version": 1,
            "id": "org.lora.wordcount",
            "version": "1.1.0",
            "kind": "wasm-component",
            "host_api": { "major": 1, "minimum_minor": 0 },
            "entry": "component.wasm",
            "exports": ["tool.wordcount.v1"],
            "requests": ["input.text"],
            "dependencies": []
        },
        "package_sha256": "sha256_wordcount_v2",
        "lock_sha256": "sha256_wordcount_v2"
    })
    .to_string();
    let r26 = send_http(&format!(
        "POST /v1/plugins/stage HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_plugin_update}",
        body_plugin_update.len()
    ));
    assert!(r26.starts_with("HTTP/1.1 200 OK"));

    // (x) POST /v1/plugins/{id}/fault records fault
    let r27 = send_http(&format!(
        "POST /v1/plugins/org.lora.wordcount/fault HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: 2\r\n\r\n{{}}"
    ));
    assert!(r27.starts_with("HTTP/1.1 200 OK"));
    assert!(r27.contains("\"status\":\"fault_recorded\""));

    // (y) POST /v1/plugins/{id}/rollback reverts to previous compatible digest
    let r28 = send_http(&format!(
        "POST /v1/plugins/org.lora.wordcount/rollback HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: 2\r\n\r\n{{}}"
    ));
    assert!(r28.starts_with("HTTP/1.1 200 OK"));
    assert!(r28.contains("\"status\":\"rolled_back\""));
    assert!(r28.contains("\"previous_digest\":\"sha256_wordcount_v1\""));

    // (z) POST /v1/adapter/evaluate verifies promotion gate
    let body_eval = serde_json::json!({
        "job_id": "eval-job-001",
        "base_model_sha256": "sha256:llama-base",
        "installed_base_sha256": "sha256:llama-base",
        "dataset_sha256": "sha256:dataset-split",
        "base_task_score_pct": 72,
        "adapter_task_score_pct": 88,
        "general_regression_pct": 0,
        "schema_valid_pct": 100,
        "target_runtime_load_verified": true
    })
    .to_string();
    let r29 = send_http(&format!(
        "POST /v1/adapter/evaluate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_eval}",
        body_eval.len()
    ));
    assert!(r29.starts_with("HTTP/1.1 200 OK"));
    assert!(r29.contains("\"decision\":\"promote_experimental\""));
    assert!(r29.contains("\"job_id\":\"eval-job-001\""));

    // (aa) POST /v1/adapter/dataset-split validates leakage-free source-grouped dataset splits
    let body_split = serde_json::json!({
        "license": "Apache-2.0",
        "examples": [
            {
                "example_id": "ex1",
                "source_group_id": "group_alpha",
                "prompt": "Prompt A",
                "target_completion": "Completion A"
            },
            {
                "example_id": "ex2",
                "source_group_id": "group_beta",
                "prompt": "Prompt B",
                "target_completion": "Completion B"
            },
            {
                "example_id": "ex3",
                "source_group_id": "group_gamma",
                "prompt": "Prompt C",
                "target_completion": "Completion C"
            }
        ]
    })
    .to_string();
    let r30 = send_http(&format!(
        "POST /v1/adapter/dataset-split HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_split}",
        body_split.len()
    ));
    assert!(r30.starts_with("HTTP/1.1 200 OK"));
    assert!(r30.contains("\"dataset_sha256\":"));
    assert!(r30.contains("\"license\":\"Apache-2.0\""));
    assert!(r30.contains("\"train_examples\":"));
    assert!(r30.contains("\"validation_examples\":"));
    assert!(r30.contains("\"test_examples\":"));

    // (ab) POST /v1/hardware/tune evaluates and stores auto-tuning receipt
    let body_tune = serde_json::json!({
        "model_alias": "api-model",
        "context_length": 2048,
        "profile": "automatic"
    })
    .to_string();
    let r31 = send_http(&format!(
        "POST /v1/hardware/tune HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_tune}",
        body_tune.len()
    ));
    assert!(r31.starts_with("HTTP/1.1 200 OK"));
    assert!(r31.contains("\"optimal_threads\":"));
    assert!(r31.contains("\"optimal_batch_size\":"));
    assert!(r31.contains("\"key_digest\":"));

    // (ac) GET /v1/hardware/tuning-receipts lists stored receipts
    let r32 = send_http(&format!(
        "GET /v1/hardware/tuning-receipts HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r32.starts_with("HTTP/1.1 200 OK"));
    assert!(r32.contains("\"key_digest\":"));
    assert!(r32.contains("\"optimal_threads\":"));

    // (ad) GET /v1/audit/events lists recorded audit events
    let r33 = send_http(&format!(
        "GET /v1/audit/events HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r33.starts_with("HTTP/1.1 200 OK"));
    assert!(r33.contains('['));

    // (ae) POST /v1/audit/prune prunes older events
    let body_prune = serde_json::json!({
        "keep_latest": 50
    })
    .to_string();
    let r34 = send_http(&format!(
        "POST /v1/audit/prune HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_prune}",
        body_prune.len()
    ));
    assert!(r34.starts_with("HTTP/1.1 200 OK"));
    assert!(r34.contains("\"status\":\"pruned\""));
    assert!(r34.contains("\"keep_latest\":50"));

    // (af) GET /v1/audit/export exports redacted audit diagnostic bundle
    let r35 = send_http(&format!(
        "GET /v1/audit/export HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r35.starts_with("HTTP/1.1 200 OK"));
    assert!(r35.contains('['));

    // (ag) POST /v1/egress/preview computes preview digest for cloud egress consent
    let body_egress_prev = serde_json::json!({
        "provider_id": "openai-cloud",
        "model_id": "gpt-4o",
        "destination_origin": "https://api.openai.com",
        "data_classes": ["user_prompt"],
        "prompt_text": "Summarize deployment guide",
        "attached_excerpts": ["CONFIG=prod"],
        "estimated_cost_usd_micros": 1200
    })
    .to_string();
    let r36 = send_http(&format!(
        "POST /v1/egress/preview HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_egress_prev}",
        body_egress_prev.len()
    ));
    assert!(r36.starts_with("HTTP/1.1 200 OK"));
    let prev_resp: serde_json::Value =
        serde_json::from_str(r36.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let approved_digest = prev_resp["preview_digest"].as_str().unwrap().to_string();

    // (ah) POST /v1/egress/verify verifies payload against human-approved EgressPermit
    let body_egress_ver = serde_json::json!({
        "permit": {
            "permit_id": "permit-live-01",
            "principal": "loopback-api-client",
            "approved_preview_digest": approved_digest,
            "expires_at_ms": 253402300799000_u64
        },
        "actual_payload": {
            "provider_id": "openai-cloud",
            "model_id": "gpt-4o",
            "destination_origin": "https://api.openai.com",
            "data_classes": ["user_prompt"],
            "prompt_text": "Summarize deployment guide",
            "attached_excerpts": ["CONFIG=prod"],
            "estimated_cost_usd_micros": 1200
        }
    })
    .to_string();
    let r37 = send_http(&format!(
        "POST /v1/egress/verify HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_egress_ver}",
        body_egress_ver.len()
    ));
    assert!(r37.starts_with("HTTP/1.1 200 OK"));
    assert!(r37.contains("\"status\":\"verified\""));
    assert!(r37.contains("\"permit_id\":\"permit-live-01\""));

    let status2 = child2.wait().unwrap();
    assert!(status2.success());
}
