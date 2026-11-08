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
    assert!(openapi_str.contains("\"/v1/catalog/validate\""));
    assert!(openapi_str.contains("\"/v1/context-budget\""));
    assert!(openapi_str.contains("\"/v1/broker/validate\""));
    assert!(openapi_str.contains("\"/v1/doctor\""));
    assert!(openapi_str.contains("\"/v1/models/classify\""));
    assert!(openapi_str.contains("\"/v1/agent/handoff\""));
    assert!(openapi_str.contains("\"/v1/workflow/run\""));
    assert!(openapi_str.contains("\"/v1/workflow/{id}/cancel\""));
    assert!(openapi_str.contains("\"/v1/workflow/verify\""));
    assert!(openapi_str.contains("\"/v1/workflow/bundle\""));
    assert!(openapi_str.contains("\"/v1/workflow/bundle/verify\""));

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
    assert!(resp.contains("\"catalog.validate\""));
    assert!(resp.contains("\"context_budget.evaluate\""));
    assert!(resp.contains("\"broker.validate\""));
    assert!(resp.contains("\"agent.handoff\""));
    assert!(resp.contains("\"models.classify\""));
    assert!(resp.contains("\"system.doctor\""));
    assert!(resp.contains("\"workflow.run\""));
    assert!(resp.contains("\"workflow.cancel\""));

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
            "60",
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

    // (ai) POST /v1/catalog/validate checks GGUF format, immutable revision, strong ETag, and HTTPS
    let body_cat_valid = serde_json::json!({
        "descriptor": {
            "model_id": "meta-llama/Llama-3-8B-Instruct",
            "immutable_revision": "e1245ff72d00f682cb304a9d70104191db48d5d4",
            "filename": "llama-3-8b.gguf",
            "size_bytes": 4800000000_u64,
            "license": "Llama-3-Community",
            "expected_sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            "strong_etag": "\"5f8a-5b12\"",
            "download_url": "https://huggingface.co/models/llama-3-8b.gguf"
        },
        "redirect_hops": 1
    })
    .to_string();
    let r38 = send_http(&format!(
        "POST /v1/catalog/validate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_cat_valid}",
        body_cat_valid.len()
    ));
    assert!(r38.starts_with("HTTP/1.1 200 OK"));
    assert!(r38.contains("\"status\":\"valid\""));
    assert!(r38.contains("\"filename\":\"llama-3-8b.gguf\""));

    // (aj) POST /v1/catalog/validate rejects unsafe script/pickle extensions
    let body_cat_bad = serde_json::json!({
        "descriptor": {
            "model_id": "meta-llama/Llama-3-8B-Instruct",
            "immutable_revision": "e1245ff72d00f682cb304a9d70104191db48d5d4",
            "filename": "malicious_loader.py",
            "size_bytes": 1024,
            "license": "Apache-2.0",
            "expected_sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            "strong_etag": "\"5f8a\"",
            "download_url": "https://huggingface.co/models/malicious_loader.py"
        }
    })
    .to_string();
    let r39 = send_http(&format!(
        "POST /v1/catalog/validate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_cat_bad}",
        body_cat_bad.len()
    ));
    assert!(r39.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r39.contains("policy_denied"));

    // (ak) POST /v1/context-budget computes deterministic token allocation
    let body_ctx_valid = serde_json::json!({
        "context_window_tokens": 2048,
        "reserved_output_tokens": 512,
        "control_schema_overhead_tokens": 128,
        "user_request_tokens": 256,
        "recent_turns_tokens": 128,
        "tool_receipts_tokens": 64,
        "ranked_excerpts": []
    })
    .to_string();
    let r40 = send_http(&format!(
        "POST /v1/context-budget HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_ctx_valid}",
        body_ctx_valid.len()
    ));
    assert!(r40.starts_with("HTTP/1.1 200 OK"));
    assert!(r40.contains("\"total_allocated_tokens\":1088"));
    assert!(r40.contains("\"remaining_headroom_tokens\":960"));

    // (al) POST /v1/context-budget fails closed when user request exceeds budget
    let body_ctx_overflow = serde_json::json!({
        "context_window_tokens": 512,
        "reserved_output_tokens": 256,
        "control_schema_overhead_tokens": 128,
        "user_request_tokens": 256,
        "recent_turns_tokens": 0,
        "tool_receipts_tokens": 0,
        "ranked_excerpts": []
    })
    .to_string();
    let r41 = send_http(&format!(
        "POST /v1/context-budget HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_ctx_overflow}",
        body_ctx_overflow.len()
    ));
    assert!(r41.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r41.contains("budget_exhausted"));

    // (am) POST /v1/broker/validate verifies single-use challenge-bound permit
    let broker_verb = serde_json::json!({
        "restart_allowlisted_service": {
            "service_name": "lora-local-service"
        }
    });
    let now = lora_core::now_epoch_ms();
    let nonce = "challenge-nonce-tcp-01";
    let sess = "sess-tcp-01";
    let permit_verb: lora_core::BrokerVerb = serde_json::from_value(broker_verb.clone()).unwrap();
    let digest =
        lora_core::BrokerSingleUsePermit::compute_request_digest(sess, nonce, &permit_verb);
    let body_broker = serde_json::json!({
        "permit": {
            "permit_id": "permit-broker-01",
            "session_id": sess,
            "challenge_nonce": nonce,
            "verb": broker_verb,
            "request_digest": digest,
            "expires_at_ms": now + 60_000
        },
        "allowlisted_services": ["lora-local-service"]
    })
    .to_string();
    let r42 = send_http(&format!(
        "POST /v1/broker/validate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_broker}",
        body_broker.len()
    ));
    assert!(r42.starts_with("HTTP/1.1 200 OK"));
    assert!(r42.contains("\"status\":\"completed\""));
    assert!(r42.contains("\"valid\":true"));

    // (an) POST /v1/broker/validate rejects replay of consumed nonce
    let r43 = send_http(&format!(
        "POST /v1/broker/validate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_broker}",
        body_broker.len()
    ));
    assert!(r43.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r43.contains("policy_denied"));
    assert!(r43.contains("replay detected"));

    // (ao) GET /v1/doctor returns holistic system health and diagnostic report
    let r44 = send_http(&format!(
        "GET /v1/doctor HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r44.starts_with("HTTP/1.1 200 OK"));
    assert!(r44.contains("\"status\":\"healthy\""));
    assert!(r44.contains("\"store_lock\":\"acquired_cleanly\""));
    assert!(r44.contains("physical_cores"));

    // (ap) POST /v1/models/classify classifies valid GGUF text model as compatible
    let body_classify_ok = serde_json::json!({
        "filename": "qwen2.5-coder-7b-q4_k_m.gguf"
    })
    .to_string();
    let r45 = send_http(&format!(
        "POST /v1/models/classify HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_classify_ok}",
        body_classify_ok.len()
    ));
    assert!(r45.starts_with("HTTP/1.1 200 OK"));
    assert!(r45.contains("\"status\":\"compatible\""));
    assert!(r45.contains("\"supported\":true"));

    // (aq) POST /v1/models/classify rejects incompatible model formats
    let body_classify_bad = serde_json::json!({
        "filename": "model.safetensors"
    })
    .to_string();
    let r46 = send_http(&format!(
        "POST /v1/models/classify HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_classify_bad}",
        body_classify_bad.len()
    ));
    assert!(r46.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r46.contains("incompatible_model"));
    assert!(r46.contains("\"supported\":false"));

    // (ar) POST /v1/agent/handoff validates specialist sub-agent delegation envelope
    let body_handoff_ok = serde_json::json!({
        "handoff": {
            "parent_run_id": "run-p01",
            "child_run_id": "run-c01",
            "objective": "Scoped evidence search in docs",
            "result_schema": "SpecialistEvidenceV1",
            "depth": 1,
            "max_depth": 2,
            "remaining_tool_calls": 3,
            "remaining_tokens": 1024,
            "parent_allowed_roots": ["docs"],
            "requested_allowed_roots": ["docs"],
            "visited_run_ids": ["run-p01"]
        }
    })
    .to_string();
    let r47 = send_http(&format!(
        "POST /v1/agent/handoff HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_handoff_ok}",
        body_handoff_ok.len()
    ));
    assert!(r47.starts_with("HTTP/1.1 200 OK"));
    assert!(r47.contains("\"status\":\"verified\""));
    assert!(r47.contains("\"admitted\":true"));

    // (as) POST /v1/agent/handoff fails closed when cycle is detected
    let body_handoff_cycle = serde_json::json!({
        "handoff": {
            "parent_run_id": "run-p01",
            "child_run_id": "run-c01",
            "objective": "Cyclic handoff",
            "result_schema": "SpecialistEvidenceV1",
            "depth": 1,
            "max_depth": 2,
            "remaining_tool_calls": 3,
            "remaining_tokens": 1024,
            "parent_allowed_roots": ["docs"],
            "requested_allowed_roots": ["docs"],
            "visited_run_ids": ["run-p01", "run-c01"]
        }
    })
    .to_string();
    let r48 = send_http(&format!(
        "POST /v1/agent/handoff HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_handoff_cycle}",
        body_handoff_cycle.len()
    ));
    assert!(r48.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r48.contains("budget_exhausted"));
    assert!(r48.contains("cycle detected"));

    // (at) POST /v1/workflow/run executes composed pipeline
    let body_wf_ok = serde_json::json!({
        "workflow": {
            "workflow_id": "wf-test-01",
            "title": "Verification Pipeline",
            "steps": [
                {
                    "index_workspace": {
                        "workspace_id": "ws-default",
                        "relative_path": "docs/ARCHITECTURE.md"
                    }
                },
                {
                    "grounded_query": {
                        "query": "What is the policy gate revision?",
                        "model": "qwen2.5-coder-7b",
                        "context_tokens": 2048
                    }
                },
                {
                    "preview_patch": {
                        "target_file": "docs/TEST.md",
                        "replacement": "updated content"
                    }
                },
                {
                    "audit_export": {
                        "output_path": "audit_pipeline.json"
                    }
                }
            ],
            "stop_on_failure": true
        }
    })
    .to_string();
    let r49 = send_http(&format!(
        "POST /v1/workflow/run HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_wf_ok}",
        body_wf_ok.len()
    ));
    assert!(r49.starts_with("HTTP/1.1 200 OK"));
    assert!(r49.contains("\"total_steps\":4"));
    assert!(r49.contains("\"completed_steps\":4"));
    assert!(r49.contains("\"successful\":true"));
    assert!(r49.contains("\"total_duration_ms\":"));
    assert!(r49.contains("\"duration_ms\":"));
    assert!(r49.contains("\"deadline_exceeded\":false"));
    assert!(r49.contains("\"attempts\":1"));
    assert!(r49.contains("\"non_fatal\":false"));

    // (au) POST /v1/workflow/run rejects empty steps list
    let body_wf_bad = serde_json::json!({
        "workflow": {
            "workflow_id": "wf-empty",
            "title": "Empty steps",
            "steps": [],
            "stop_on_failure": true
        }
    })
    .to_string();
    let r50 = send_http(&format!(
        "POST /v1/workflow/run HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_wf_bad}",
        body_wf_bad.len()
    ));
    assert!(r50.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r50.contains("policy_denied"));

    // (av) POST /v1/workflow/{id}/cancel cancels workflow
    let r51 = send_http(&format!(
        "POST /v1/workflow/wf-test-01/cancel HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: 0\r\n\r\n"
    ));
    assert!(r51.starts_with("HTTP/1.1 200 OK"));
    assert!(r51.contains("\"workflow_id\":\"wf-test-01\""));
    assert!(r51.contains("\"status\":\"canceled\""));

    // (aw) GET /v1/workflow/receipts lists persisted workflow execution receipts
    let r52 = send_http(&format!(
        "GET /v1/workflow/receipts HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r52.starts_with("HTTP/1.1 200 OK"));
    assert!(r52.contains("\"count\":"));
    assert!(r52.contains("\"workflow_id\":\"wf-test-01\""));
    assert!(r52.contains("\"total_duration_ms\":"));
    assert!(r52.contains("\"deadline_exceeded\":false"));

    // (ax) GET /v1/workflow/receipts/{id} returns specific persisted receipt
    let r53 = send_http(&format!(
        "GET /v1/workflow/receipts/wf-test-01 HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r53.starts_with("HTTP/1.1 200 OK"));
    assert!(r53.contains("\"found\":true"));
    assert!(r53.contains("\"total_steps\":4"));
    assert!(r53.contains("\"workflow_id\":\"wf-test-01\""));
    assert!(r53.contains("\"total_duration_ms\":"));
    assert!(r53.contains("\"deadline_exceeded\":false"));

    // (ay) GET /v1/workflow/receipts/{id} returns 404 for non-existent receipt
    let r54 = send_http(&format!(
        "GET /v1/workflow/receipts/non-existent-wf-999 HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r54.starts_with("HTTP/1.1 404 Not Found"));
    assert!(r54.contains("receipt_not_found"));

    // (az) POST /v1/workflow/receipts/prune prunes workflow receipts retaining keep_latest
    let body_prune = serde_json::json!({ "keep_latest": 10 }).to_string();
    let r55 = send_http(&format!(
        "POST /v1/workflow/receipts/prune HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_prune}",
        body_prune.len()
    ));
    assert!(r55.starts_with("HTTP/1.1 200 OK"));
    assert!(r55.contains("\"deleted\":"));
    assert!(r55.contains("\"retained\":10"));

    // (ba) POST /v1/workflow/validate dry-run checks pipeline parameters
    let r56 = send_http(&format!(
        "POST /v1/workflow/validate HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_wf_ok}",
        body_wf_ok.len()
    ));
    assert!(r56.starts_with("HTTP/1.1 200 OK"));
    assert!(r56.contains("\"valid\":true"));
    assert!(r56.contains("\"total_steps\":4"));
    assert!(r56.contains("\"config_sha256\":"));

    // (bb) POST /v1/workflow/verify verifies persisted workflow receipt attestation
    let body_verify_ok = serde_json::json!({
        "workflow_id": "wf-test-01"
    })
    .to_string();
    let r57 = send_http(&format!(
        "POST /v1/workflow/verify HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_verify_ok}",
        body_verify_ok.len()
    ));
    assert!(r57.starts_with("HTTP/1.1 200 OK"));
    assert!(r57.contains("\"verified\":true"));
    assert!(r57.contains("\"mode\":\"receipt_self_integrity\""));
    assert!(r57.contains("\"workflow_id\":\"wf-test-01\""));

    // (bc) POST /v1/workflow/verify fails closed when config does not match persisted receipt
    let body_verify_bad = serde_json::json!({
        "workflow_id": "wf-test-01",
        "workflow": {
            "workflow_id": "wf-test-01",
            "title": "Tampered Title",
            "steps": [],
            "stop_on_failure": true
        }
    })
    .to_string();
    let r58 = send_http(&format!(
        "POST /v1/workflow/verify HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{body_verify_bad}",
        body_verify_bad.len()
    ));
    assert!(r58.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(r58.contains("digest_mismatch"));

    // (bd) GET /v1/workflow/bundle exports portable WorkflowAttestationBundle
    let r59 = send_http(&format!(
        "GET /v1/workflow/bundle HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\n\r\n"
    ));
    assert!(r59.starts_with("HTTP/1.1 200 OK"));
    assert!(r59.contains("\"schema_version\":1"));
    assert!(r59.contains("\"bundle_sha256\":"));
    let bundle_json_str = r59.split("\r\n\r\n").nth(1).unwrap();

    // (be) POST /v1/workflow/bundle/verify verifies exported bundle
    let r60 = send_http(&format!(
        "POST /v1/workflow/bundle/verify HTTP/1.1\r\nHost: {bind_addr2}\r\nAuthorization: Bearer live-secret-token-42\r\nContent-Length: {}\r\n\r\n{bundle_json_str}",
        bundle_json_str.len()
    ));
    assert!(r60.starts_with("HTTP/1.1 200 OK"));
    assert!(r60.contains("\"verified\":true"));

    let status2 = child2.wait().unwrap();
    assert!(status2.success());
}
