//! Post-MVP Acceptance Suite covering:
//! - Phase 6: Scoped Memory & SQLite FTS5 Retrieval (`docs/MEMORY_AND_RAG.md`)
//! - Phase 6: Extension & Plugin Lifecycle, Quarantine & Rollback (`docs/PLUGIN_SYSTEM.md`)
//! - Phase 6: Loopback API Security (`R08`), Idempotency & Cloud Egress Consent (`docs/API_AND_SDK.md`)
//! - Phase 8: Adapter Studio Leakage-Free Splits & Promotion Gate (`docs/TRAINING_SYSTEM.md`)

use lora_core::{
    anonymize_public_benchmark_report, apply_approved_patch, classify_artifact_compatibility,
    compute_unified_memory_budget, evaluate_adapter_promotion_gate, evaluate_provider_retry_policy,
    evaluate_range_resume, keyed_correlation_digest, prepare_leakage_free_splits,
    preview_checked_patch, rollback_applied_patch, slice_virtualized_transcript,
    validate_catalog_variant, validate_loopback_bind, verify_egress_permit,
    verify_loopback_http_security, AdapterPromotionDecision, AdmissionPlanner, ApiRunManager,
    ApprovalRequest, BrokerSingleUsePermit, BrokerStatus, BrokerVerb, CatalogVariantDescriptor,
    ChargingState, DownloadResumeCheckpoint, EgressConsentPreview, EgressPermit,
    ExtensionHostManager, ExtensionManifestV1, ExtensionState, HeldOutEvaluationReceipt,
    HostApiVersion, HostSnapshot, IdempotencyLedger, LoraError, LoraJobConfig, PatchProposal,
    PolicyGate, PrivilegedBrokerGate, ProviderFailureStage, ProviderRetryDecision, ReasonCode,
    ResumeDecision, ScopedFileExecutorPort, SqliteStore, StageLatencyProfile, TrainingExample,
    TrustedConsentDialogViewModel, TuningCacheKey, TuningProfile, TuningTrialSample,
    DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_platform::OsScopedFileExecutor;
use std::fs;
use tempfile::TempDir;

#[test]
fn phase6_memory_fts5_ingestion_isolation_and_forget() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("rag.db");
    let ws_a = tmp.path().join("project_a");
    let ws_b = tmp.path().join("project_b");
    fs::create_dir_all(&ws_a).unwrap();
    fs::create_dir_all(&ws_b).unwrap();

    // Two workspaces with identical filenames (`notes.md`) plus a secret `.env` file
    fs::write(
        ws_a.join("notes.md"),
        "# Architecture\nProject Alpha uses sapphire_scheduler_v2 for batch routing.\n",
    )
    .unwrap();
    fs::write(ws_a.join(".env"), "SECRET_KEY=CANARY_SECRET_ALPHA").unwrap();

    fs::write(
        ws_b.join("notes.md"),
        "# Architecture\nProject Beta uses crimson_dispatcher_v7 for batch routing.\n",
    )
    .unwrap();

    let executor = OsScopedFileExecutor;
    let root_a = executor.register_root(&ws_a).unwrap();
    let root_b = executor.register_root(&ws_b).unwrap();

    let policy = PolicyGate::new();
    policy.register_scoped_root(root_a.clone());
    policy.register_scoped_root(root_b.clone());

    let store = SqliteStore::open(&db_path).unwrap();

    let receipt_a = store
        .index_scoped_files(
            &root_a,
            &["notes.md".to_string(), ".env".to_string()],
            32 * 1024,
            &executor,
            &|| Ok(()),
        )
        .expect("index project A");
    assert_eq!(receipt_a.indexed_files, 1);
    assert_eq!(
        receipt_a.skipped_secret_or_invalid,
        vec![".env".to_string()]
    );

    store
        .index_scoped_files(
            &root_b,
            &["notes.md".to_string()],
            32 * 1024,
            &executor,
            &|| Ok(()),
        )
        .expect("index project B");

    // 1. Search in Workspace A returns only Project Alpha's `notes.md`
    let hits_a = store
        .search_lexical_fts(&root_a.root_id, "sapphire_scheduler_v2", 5, &policy)
        .unwrap();
    assert_eq!(hits_a.len(), 1);
    assert_eq!(hits_a[0].relative_path, "notes.md");
    assert_eq!(hits_a[0].line_range, (1, 2));

    let cross_check = store
        .search_lexical_fts(&root_a.root_id, "crimson_dispatcher_v7", 5, &policy)
        .unwrap();
    assert!(
        cross_check.is_empty(),
        "Workspace A must never see Workspace B chunks"
    );

    // 2. Forget Workspace A: immediately tombstones and deletes derived FTS5/chunk entries
    let deleted_count = store.forget_workspace_index(&root_a.root_id).unwrap();
    assert!(deleted_count >= 1);

    // Verify even across a fresh SQLite connection after restart that Workspace A returns 0 hits
    let reopened_store = SqliteStore::open(&db_path).unwrap();
    let post_forget = reopened_store
        .search_lexical_fts(&root_a.root_id, "sapphire_scheduler_v2", 5, &policy)
        .unwrap();
    assert!(post_forget.is_empty());

    // 3. Revoking all grants prevents search dispatch
    policy.revoke_all_grants();
    let revoked_search =
        reopened_store.search_lexical_fts(&root_b.root_id, "crimson_dispatcher_v7", 5, &policy);
    assert!(matches!(
        revoked_search,
        Err(LoraError::GrantRevokedOrExpired)
    ));
}

#[test]
fn phase6_extension_lifecycle_quarantine_and_rollback() {
    let mut host = ExtensionHostManager::new();

    let manifest_v1 = ExtensionManifestV1 {
        manifest_version: 1,
        id: "org.example.wordcount".to_string(),
        version: "1.0.0".to_string(),
        kind: "wasm-component".to_string(),
        host_api: HostApiVersion {
            major: 1,
            minimum_minor: 0,
        },
        entry: "component.wasm".to_string(),
        exports: vec!["tool.wordcount.v1".to_string()],
        requests: vec!["input.text".to_string()],
        dependencies: vec![],
    };

    // 1. Stage and verify v1.0.0
    let state = host
        .stage_and_verify(manifest_v1.clone(), "sha256_v100", "sha256_v100")
        .unwrap();
    assert_eq!(state, ExtensionState::Disabled);

    // 2. Activate with approved capability
    let active = host
        .activate("org.example.wordcount", &["input.text".to_string()])
        .unwrap();
    assert_eq!(active, ExtensionState::Active);

    // 3. Stage update v1.2.0 requesting an additional capability
    let mut manifest_v12 = manifest_v1.clone();
    manifest_v12.version = "1.2.0".to_string();
    manifest_v12.requests.push("workspace.summary".to_string());
    host.stage_and_verify(manifest_v12, "sha256_v120", "sha256_v120")
        .unwrap();

    let diff = host.permission_diff("org.example.wordcount").unwrap();
    assert!(diff.contains(&"workspace.summary".to_string()));

    // 4. Simulate 3 consecutive runtime faults -> automatically transitions to Quarantined
    host.activate(
        "org.example.wordcount",
        &["input.text".to_string(), "workspace.summary".to_string()],
    )
    .unwrap();
    host.record_fault("org.example.wordcount").unwrap();
    host.record_fault("org.example.wordcount").unwrap();
    let q_state = host.record_fault("org.example.wordcount").unwrap();
    assert_eq!(q_state, ExtensionState::Quarantined);

    // Quarantined extension cannot be directly re-activated without review/rollback
    assert!(host
        .activate(
            "org.example.wordcount",
            &["input.text".to_string(), "workspace.summary".to_string()]
        )
        .is_err());

    // 5. Roll back to previous compatible digest (`sha256_v100`)
    let rolled_back_sha = host.rollback("org.example.wordcount").unwrap();
    assert_eq!(rolled_back_sha, "sha256_v100");
    assert_eq!(
        host.get("org.example.wordcount").unwrap().state,
        ExtensionState::Disabled
    );

    // 6. Reject ambient capability requests and dependency cycles
    let mut bad_ambient = manifest_v1.clone();
    bad_ambient.id = "org.evil.plugin".to_string();
    bad_ambient.requests = vec!["ambient.fs".to_string()];
    assert_eq!(
        host.stage_and_verify(bad_ambient, "sha_bad", "sha_bad")
            .unwrap_err()
            .reason_code(),
        ReasonCode::PolicyDenied
    );

    let mut cyc_a = manifest_v1.clone();
    cyc_a.id = "ext.a".to_string();
    cyc_a.dependencies = vec!["ext.b".to_string()];
    let mut cyc_b = manifest_v1;
    cyc_b.id = "ext.b".to_string();
    cyc_b.dependencies = vec!["ext.a".to_string()];
    host.stage_and_verify(cyc_a, "sha_a", "sha_a").unwrap();
    let cyc_err = host
        .stage_and_verify(cyc_b, "sha_b", "sha_b")
        .expect_err("dependency cycle must be rejected");
    assert_eq!(cyc_err.reason_code(), ReasonCode::MalformedArtifact);

    // 7. Persist and reload ExtensionHostManager via SQLite
    let tmp = TempDir::new().expect("tempdir");
    let store = SqliteStore::open(&tmp.path().join("ext.db")).unwrap();
    store.save_extension_host(&host).unwrap();
    let reloaded = store.load_extension_host().unwrap();
    assert_eq!(reloaded.list().len(), 2);
    assert_eq!(
        reloaded
            .get("org.example.wordcount")
            .unwrap()
            .package_sha256,
        "sha256_v100"
    );

    // 8. Explicit disable and capability revocation
    host.activate(
        "org.example.wordcount",
        &["input.text".to_string(), "workspace.summary".to_string()],
    )
    .unwrap();
    assert_eq!(
        host.get("org.example.wordcount").unwrap().state,
        ExtensionState::Active
    );
    let dis_state = host.disable_and_revoke("org.example.wordcount").unwrap();
    assert_eq!(dis_state, ExtensionState::Disabled);
    assert!(host
        .get("org.example.wordcount")
        .unwrap()
        .approved_capabilities
        .is_empty());
}

#[test]
fn phase6_loopback_api_security_and_cloud_egress_consent() {
    // 1. Only numeric loopback bind addresses allowed by default
    assert!(validate_loopback_bind("127.0.0.1:8421").is_ok());
    assert!(validate_loopback_bind("[::1]:8421").is_ok());
    assert!(validate_loopback_bind("0.0.0.0:8421").is_err());

    // 2. Valid loopback request passes
    let token = "sec-token-998877";
    assert!(verify_loopback_http_security(
        Some("127.0.0.1:8421"),
        Some("tauri://localhost"),
        Some("Bearer sec-token-998877"),
        None,
        "/v1/runs",
        token,
    )
    .is_ok());

    // 3. DNS rebinding (`Host: attacker.dns-rebind.invalid`) blocked (`R08`)
    assert!(verify_loopback_http_security(
        Some("attacker.dns-rebind.invalid:8421"),
        None,
        Some("Bearer sec-token-998877"),
        None,
        "/v1/runs",
        token,
    )
    .is_err());

    // 4. Cross-origin hostile browser tab (`Origin: https://evil.example`) blocked (`R08`)
    assert!(verify_loopback_http_security(
        Some("127.0.0.1:8421"),
        Some("https://evil.example"),
        Some("Bearer sec-token-998877"),
        None,
        "/v1/runs",
        token,
    )
    .is_err());

    // 5. Token in URL query string or ambient Cookie rejected
    assert!(verify_loopback_http_security(
        Some("127.0.0.1:8421"),
        None,
        Some("Bearer sec-token-998877"),
        None,
        "/v1/runs?access_token=sec-token-998877",
        token,
    )
    .is_err());
    assert!(verify_loopback_http_security(
        Some("127.0.0.1:8421"),
        None,
        Some("Bearer sec-token-998877"),
        Some("session_id=ambient123"),
        "/v1/runs",
        token,
    )
    .is_err());

    // 6. Idempotency key reuse returns same run ID on identical body, and conflicts (409) on modified body
    let mut ledger = IdempotencyLedger::new();
    let run1 = ledger
        .register_or_verify(
            "mina",
            "idem-key-1",
            r#"{"model":"m1","prompt":"hello"}"#,
            "run-101",
        )
        .unwrap();
    assert_eq!(run1, "run-101");
    let run1_repeat = ledger
        .register_or_verify(
            "mina",
            "idem-key-1",
            r#"{"model":"m1","prompt":"hello"}"#,
            "run-102",
        )
        .unwrap();
    assert_eq!(run1_repeat, "run-101");
    assert!(ledger
        .register_or_verify(
            "mina",
            "idem-key-1",
            r#"{"model":"m1","prompt":"DIFFERENT"}"#,
            "run-103",
        )
        .is_err());

    // 7. Cloud Provider Egress Consent preview & payload tampering detection
    let preview = EgressConsentPreview {
        provider_id: "remote-openai-compat".to_string(),
        model_id: "cloud-model-v1".to_string(),
        destination_origin: "https://api.provider.example".to_string(),
        data_classes: vec!["prompt_text".to_string(), "file_excerpt".to_string()],
        prompt_text: "Summarize public release notes".to_string(),
        attached_excerpts: vec!["guide.md#L1-L5".to_string()],
        estimated_cost_usd_micros: Some(150),
    };
    let permit = EgressPermit {
        permit_id: "egress-1".to_string(),
        principal: "mina".to_string(),
        approved_preview_digest: preview.compute_preview_digest(),
        expires_at_ms: 2_000_000,
    };
    assert!(verify_egress_permit(&permit, &preview, 1_000_000).is_ok());

    // Changing destination or attached excerpts invalidates the egress permit
    let mut tampered = preview.clone();
    tampered
        .attached_excerpts
        .push("secrets.env#L1-L2".to_string());
    assert!(verify_egress_permit(&permit, &tampered, 1_000_000).is_err());

    // 8. ApiRunManager: owner-only isolation, Last-Event-ID SSE replay window, replay_gap, and idempotent cancel
    let mut run_mgr = ApiRunManager::new(2);
    let caps = run_mgr.capabilities_for("mina");
    assert_eq!(caps.api_major, 1);
    assert_eq!(caps.sse_replay_window_events, 2);

    run_mgr.create_run("run-sse-1", "mina");
    run_mgr
        .append_event("run-sse-1", "delta", r#"{"text":"tok1"}"#)
        .unwrap();
    run_mgr
        .append_event("run-sse-1", "delta", r#"{"text":"tok2"}"#)
        .unwrap();
    run_mgr
        .append_event("run-sse-1", "delta", r#"{"text":"tok3"}"#)
        .unwrap();

    // Unauthorized cross-principal read is denied
    assert!(run_mgr.get_run("run-sse-1", "other-user").is_err());

    // Reconnecting from sequence 1 (`run-sse-1:1`) when window size is 2 (holding seq 2 and 3) succeeds without gap
    let replay_ok = run_mgr
        .replay_events("run-sse-1", "mina", Some("run-sse-1:1"))
        .unwrap();
    assert!(!replay_ok.replay_gap_detected);
    assert_eq!(replay_ok.events.len(), 2);
    assert!(replay_ok.events[0]
        .to_sse_wire()
        .contains("id: run-sse-1:2"));

    // Reconnecting from sequence 0 (`run-sse-1:0`) detects a replay gap because seq 1 expired
    let replay_gap = run_mgr
        .replay_events("run-sse-1", "mina", Some("run-sse-1:0"))
        .unwrap();
    assert!(replay_gap.replay_gap_detected);

    // Idempotent cancellation transitions run to cancelled and blocks late frames
    let cancelled = run_mgr.cancel_run("run-sse-1", "mina").unwrap();
    assert_eq!(cancelled.state, "cancelled");
    let cancelled_again = run_mgr.cancel_run("run-sse-1", "mina").unwrap();
    assert_eq!(cancelled_again.state, "cancelled");
    assert!(run_mgr
        .append_event("run-sse-1", "delta", r#"{"text":"late"}"#)
        .is_err());
}

#[test]
fn phase8_adapter_studio_leakage_free_split_and_promotion_gate() {
    let examples = vec![
        TrainingExample {
            example_id: "ex-1".to_string(),
            source_group_id: "doc-alpha".to_string(),
            prompt: "Q1".to_string(),
            target_completion: "A1".to_string(),
        },
        TrainingExample {
            example_id: "ex-2".to_string(),
            source_group_id: "doc-alpha".to_string(),
            prompt: "Q2".to_string(),
            target_completion: "A2".to_string(),
        },
        TrainingExample {
            example_id: "ex-3".to_string(),
            source_group_id: "doc-beta".to_string(),
            prompt: "Q3".to_string(),
            target_completion: "A3".to_string(),
        },
        TrainingExample {
            example_id: "ex-4".to_string(),
            source_group_id: "doc-gamma".to_string(),
            prompt: "Q4".to_string(),
            target_completion: "A4".to_string(),
        },
        TrainingExample {
            example_id: "ex-5".to_string(),
            source_group_id: "doc-delta".to_string(),
            prompt: "Q5".to_string(),
            target_completion: "A5".to_string(),
        },
    ];

    // 1. Unknown dataset license is rejected
    assert!(prepare_leakage_free_splits(&examples, "unknown").is_err());

    // 2. Valid source-grouped split guarantees zero overlap between train, validation, and test groups
    let split = prepare_leakage_free_splits(&examples, "Apache-2.0").unwrap();
    assert!(split.train_groups.is_disjoint(&split.validation_groups));
    assert!(split.train_groups.is_disjoint(&split.test_groups));
    assert!(split.validation_groups.is_disjoint(&split.test_groups));
    assert!(!split.train_examples.is_empty());
    assert!(!split.validation_examples.is_empty());
    assert!(!split.test_examples.is_empty());

    let job = LoraJobConfig {
        job_id: "lora-job-001".to_string(),
        base_model_sha256: "base_sha_12345".to_string(),
        tokenizer_template_sha256: "tpl_sha_12345".to_string(),
        dataset_sha256: split.dataset_sha256.clone(),
        rank: 16,
        alpha: 32,
        target_modules: vec!["q_proj".to_string(), "v_proj".to_string()],
        epochs: 3,
        context_length: 2048,
        microbatch_size: 4,
        seed: 42,
    };

    // 3. Mismatched base model digest is rejected
    assert!(evaluate_adapter_promotion_gate(
        &job,
        "wrong_base_sha",
        HeldOutEvaluationReceipt {
            base_task_score_pct: 70,
            adapter_task_score_pct: 85,
            general_regression_pct: 0,
            schema_valid_pct: 100,
            target_runtime_load_verified: true,
        },
    )
    .is_err());

    // 4. Adapter that improves held-out score and loads in target runtime is promoted as experimental
    let promoted = evaluate_adapter_promotion_gate(
        &job,
        "base_sha_12345",
        HeldOutEvaluationReceipt {
            base_task_score_pct: 70,
            adapter_task_score_pct: 84,
            general_regression_pct: 1,
            schema_valid_pct: 100,
            target_runtime_load_verified: true,
        },
    )
    .unwrap();
    assert_eq!(
        promoted.decision,
        AdapterPromotionDecision::PromoteExperimental
    );

    // 5. Adapter that fails target runtime load or regresses is rejected while retaining the receipt
    let rejected = evaluate_adapter_promotion_gate(
        &job,
        "base_sha_12345",
        HeldOutEvaluationReceipt {
            base_task_score_pct: 70,
            adapter_task_score_pct: 68,
            general_regression_pct: 4,
            schema_valid_pct: 100,
            target_runtime_load_verified: true,
        },
    )
    .unwrap();
    assert_eq!(
        rejected.decision,
        AdapterPromotionDecision::RejectKeepReceipt
    );
}

#[test]
fn phase4_desktop_ux_view_models_and_virtualization() {
    // 1. Trusted consent dialog defaults focus to "deny" and redacts control/secret tokens
    let req = ApprovalRequest {
        run_id: "run-desktop-1".to_string(),
        proposal_id: "prop-1".to_string(),
        tool_id: "read_file".to_string(),
        root_id: "root-abc".to_string(),
        root_display_path: "/workspace".to_string(),
        relative_path: "docs/guide.md".to_string(),
        max_bytes: 32768,
        purpose: "Answer question".to_string(),
        approval_digest: "dig-123".to_string(),
    };
    let vm = TrustedConsentDialogViewModel::from_approval_request(&req);
    assert_eq!(vm.default_button_focus, "deny");
    assert_eq!(vm.target_relative_path, "docs/guide.md");

    // 2. Virtualized transcript window slices long transcripts cleanly
    let messages: Vec<usize> = (0..1000).collect();
    let slice = slice_virtualized_transcript(&messages, 200, 25);
    assert_eq!(slice.total_items, 1000);
    assert_eq!(slice.start_index, 200);
    assert_eq!(slice.end_index, 225);
    assert_eq!(slice.visible_items.len(), 25);
}

#[test]
fn phase7_hardware_intelligence_tuning_receipts() {
    let tmp = TempDir::new().expect("tempdir");
    let store = SqliteStore::open(&tmp.path().join("tuning.db")).unwrap();

    let key = TuningCacheKey {
        hardware_fingerprint: "hw-i9-14900hx".to_string(),
        os_arch: "windows-x86_64".to_string(),
        worker_revision: "rev-1".to_string(),
        model_sha256: "mdl-sha-1".to_string(),
        context_length: 2048,
        profile: TuningProfile::Balanced,
    };

    // 1. Trial with >5% gain and low variance is promoted and persisted
    let trials = vec![
        TuningTrialSample {
            threads: 4,
            batch_size: 256,
            tok_per_sec_x100: 2000,
            variance_pct: 3,
            peak_rss_bytes: 64 * 1024 * 1024,
            thermal_or_memory_warning: false,
        },
        TuningTrialSample {
            threads: 8,
            batch_size: 512,
            tok_per_sec_x100: 2650,
            variance_pct: 4,
            peak_rss_bytes: 72 * 1024 * 1024,
            thermal_or_memory_warning: false,
        },
    ];
    let receipt = store
        .evaluate_and_store_tuning_receipt(key.clone(), 4, 256, 2000, &trials, 300_000)
        .unwrap();
    assert_eq!(receipt.optimal_threads, 8);
    assert_eq!(receipt.optimal_batch_size, 512);
    assert!(!receipt.aborted_due_to_pressure_or_thermal);

    // 2. Upgrading worker_revision invalidates cached tuning receipt (`docs/HARDWARE_INTELLIGENCE.md`)
    let mut upgraded_key = key.clone();
    upgraded_key.worker_revision = "rev-2".to_string();
    assert!(store
        .get_valid_tuning_receipt(&upgraded_key, receipt.created_at_ms + 10)
        .unwrap()
        .is_none());

    // 3. Thermal or memory warning immediately aborts tuning and keeps conservative baseline
    let hot_trials = vec![TuningTrialSample {
        threads: 16,
        batch_size: 1024,
        tok_per_sec_x100: 3500,
        variance_pct: 2,
        peak_rss_bytes: 128 * 1024 * 1024,
        thermal_or_memory_warning: true,
    }];
    let aborted = store
        .evaluate_and_store_tuning_receipt(key, 4, 256, 2000, &hot_trials, 300_000)
        .unwrap();
    assert!(aborted.aborted_due_to_pressure_or_thermal);
    assert_eq!(aborted.optimal_threads, 4);
}

#[test]
fn phase5_supervised_agent_checked_patch_and_rollback() {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();

    let initial_text = "fn compute_budget() -> u32 {\n    100\n}\n";
    fs::write(ws.join("budget.rs"), initial_text).unwrap();

    let executor = OsScopedFileExecutor;
    let root = executor.register_root(&ws).unwrap();

    let initial_read = executor
        .read_scoped_file(&root, "budget.rs", 8192, &|| Ok(()))
        .unwrap();
    let updated_text = "fn compute_budget() -> u32 {\n    250\n}\n";

    let proposal = PatchProposal {
        root_id: root.root_id.clone(),
        relative_path: "budget.rs".to_string(),
        expected_before_sha256: initial_read.sha256_digest.clone(),
        old_snippet: "    100".to_string(),
        new_snippet: "    250".to_string(),
    };

    // 1. Preview generates unified diff and verifies precondition digest
    let preview = preview_checked_patch(&root, &proposal).unwrap();
    assert!(preview.unified_diff_summary.contains("-    100"));
    assert!(preview.unified_diff_summary.contains("+    250"));

    // 2. Apply approved patch atomically and verify file contents
    let receipt = apply_approved_patch(&root, &proposal, &preview.approval_digest).unwrap();
    let after_apply = fs::read_to_string(ws.join("budget.rs")).unwrap();
    assert_eq!(after_apply, updated_text);

    // 3. Rollback using compensating receipt restores original content
    rollback_applied_patch(&root, &receipt).unwrap();
    let after_rollback = fs::read_to_string(ws.join("budget.rs")).unwrap();
    assert_eq!(after_rollback, initial_text);

    // 4. Concurrent external modification invalidates the patch precondition
    fs::write(
        ws.join("budget.rs"),
        "fn compute_budget() -> u32 {\n    999\n}\n",
    )
    .unwrap();
    let stale_err = apply_approved_patch(&root, &proposal, &preview.approval_digest).unwrap_err();
    assert_eq!(stale_err.reason_code(), ReasonCode::PolicyDenied);
}

#[test]
fn model_retirement_and_blob_garbage_collection() {
    use lora_core::{
        ApprovalDecision, ApprovalPromptPort, LoraApplicationService, ModelLifecycleState,
        DEFAULT_SAFETY_RESERVE_BYTES,
    };
    use lora_platform::{InProcessWorkerSpawner, OsStoreLockPort, SysinfoHardwareProbe};
    use lora_protocol::{build_synthetic_gguf, SyntheticGgufSpec};
    use std::sync::Arc;

    struct DenyPrompt;
    impl ApprovalPromptPort for DenyPrompt {
        fn prompt_user(&self, _req: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
            Ok(ApprovalDecision::Deny)
        }
    }

    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let fixture_path = tmp.path().join("tiny.gguf");
    fs::write(
        &fixture_path,
        build_synthetic_gguf(&SyntheticGgufSpec::default()),
    )
    .unwrap();

    let lock_port = OsStoreLockPort;
    let hw_probe = Arc::new(SysinfoHardwareProbe);
    let file_executor = Arc::new(OsScopedFileExecutor);
    let worker_spawner = Arc::new(InProcessWorkerSpawner::default());
    let approval_ui = Arc::new(DenyPrompt);

    let app = LoraApplicationService::open(
        &data_dir,
        DEFAULT_SAFETY_RESERVE_BYTES,
        &lock_port,
        hw_probe.clone(),
        file_executor,
        worker_spawner,
        approval_ui,
    )
    .unwrap();

    let imported = app
        .import_model(&fixture_path, Some("tiny-retire-test"), None)
        .unwrap();
    assert!(imported.blob_path.exists());

    // 1. Refuse deletion while an admission lease is active (pinned session revision)
    let snap = app.hw_probe.capture_snapshot(&data_dir).unwrap();
    let mut lease = app
        .planner
        .evaluate_and_reserve(
            &imported.manifest.files[0].sha256,
            &imported.manifest.probed,
            1024,
            TuningProfile::Balanced,
            &snap,
        )
        .unwrap();
    let busy_err = app.delete_model("tiny-retire-test").unwrap_err();
    assert_eq!(busy_err.reason_code(), ReasonCode::ActiveGenerationBusy);
    lease.release();

    // 2. Once lease is released, delete retires the alias and garbage-collects the unreferenced blob
    let deleted = app.delete_model("tiny-retire-test").unwrap();
    assert_eq!(deleted.final_state, ModelLifecycleState::Collected);
    assert!(deleted.blob_garbage_collected);
    assert!(!imported.blob_path.exists());
    assert!(app.registry.list_models().unwrap().is_empty());
}

#[test]
fn cli_subprocess_e2e_workflow() {
    use std::process::Command;

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("cli_data");
    let fixture = tmp.path().join("cli_fixture.gguf");
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("note.txt"), "hello world\nversion = 1\n").unwrap();

    // 1. Generate GGUF v3 fixture
    let gen_out = Command::new(lora_bin)
        .args(["generate-fixture", fixture.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(gen_out.status.success());
    assert!(fixture.exists());

    // 2. Inspect with Capri signal
    let inspect_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args(["inspect", "--signal", "capri"])
        .output()
        .unwrap();
    assert!(inspect_out.status.success());
    let inspect_stdout = String::from_utf8_lossy(&inspect_out.stdout);
    assert!(inspect_stdout.contains("NC-EDGERUNNER-02::CAPRI"));

    // 3. Import fixture and run chat
    let import_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "import",
            fixture.to_str().unwrap(),
            "--alias",
            "cli-e2e-model",
        ])
        .output()
        .unwrap();
    assert!(import_out.status.success());

    let chat_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "chat",
            "--model",
            "cli-e2e-model",
            "--prompt",
            "Hello from CLI E2E",
        ])
        .output()
        .unwrap();
    assert!(chat_out.status.success());

    // 4. Index and search workspace via CLI
    let index_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "index",
            "--scope",
            ws.to_str().unwrap(),
            "--files",
            "note.txt",
        ])
        .output()
        .unwrap();
    assert!(index_out.status.success());

    let search_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "search",
            "--scope",
            ws.to_str().unwrap(),
            "--query",
            "version",
        ])
        .output()
        .unwrap();
    assert!(search_out.status.success());
    assert!(String::from_utf8_lossy(&search_out.stdout).contains("note.txt"));

    // 5. Exercise risk-matrix, compatibility, and context-budget CLI subcommands
    let risk_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args(["risk-matrix"])
        .output()
        .unwrap();
    assert!(risk_out.status.success());
    assert!(String::from_utf8_lossy(&risk_out.stdout).contains("[R18]"));

    let compat_ok = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args(["compatibility", "--filename", "qwen-3b-q4_k_m.gguf"])
        .output()
        .unwrap();
    assert!(compat_ok.status.success());

    let compat_err = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args(["compatibility", "--filename", "model.safetensors"])
        .output()
        .unwrap();
    assert!(!compat_err.status.success());

    let budget_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "context-budget",
            "--window",
            "2048",
            "--output",
            "512",
            "--user",
            "256",
        ])
        .output()
        .unwrap();
    assert!(budget_out.status.success());
    assert!(String::from_utf8_lossy(&budget_out.stdout).contains("Remaining headroom"));

    // 6. Exercise broker and handoff-check CLI subcommands (including authority escalation rejection)
    let broker_ok = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args(["broker", "--service", "lora-local-service"])
        .output()
        .unwrap();
    assert!(broker_ok.status.success());

    let broker_denied = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args(["broker", "--service", "unlisted-root-daemon"])
        .output()
        .unwrap();
    assert!(!broker_denied.status.success());

    let handoff_ok = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "handoff-check",
            "--parent-run",
            "run-p1",
            "--child-run",
            "run-c1",
            "--parent-roots",
            "ws-a,ws-b",
            "--requested-roots",
            "ws-a",
        ])
        .output()
        .unwrap();
    assert!(handoff_ok.status.success());

    let handoff_escalated = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "handoff-check",
            "--parent-run",
            "run-p1",
            "--child-run",
            "run-c2",
            "--parent-roots",
            "ws-a",
            "--requested-roots",
            "ws-a,ws-secret",
        ])
        .output()
        .unwrap();
    assert!(!handoff_escalated.status.success());

    // 7. Import a GGUF model with a standard ChatML template ({{ message.content }}) and run `lora ask` with `--worker-backend deterministic`
    use lora_protocol::{build_synthetic_gguf, SyntheticGgufSpec};
    let chatml_gguf_path = tmp.path().join("chatml-qwen3.gguf");
    let chatml_bytes = build_synthetic_gguf(&SyntheticGgufSpec {
        architecture: "qwen3".to_string(),
        model_name: "qwen3-chatml".to_string(),
        chat_template: Some(
            "{% for message in messages %}<|im_start|>{{ message.role }}\n{{ message.content }}<|im_end|>\n{% endfor %}"
                .to_string(),
        ),
        ..SyntheticGgufSpec::default()
    });
    fs::write(&chatml_gguf_path, &chatml_bytes).unwrap();

    let import_chatml = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "--worker-backend",
            "deterministic",
            "import",
            chatml_gguf_path.to_str().unwrap(),
            "--alias",
            "qwen3-chatml",
        ])
        .output()
        .unwrap();
    assert!(import_chatml.status.success());

    fs::write(ws.join("guide.md"), "RECOMMENDED_OPTION=local_safe_mode\n").unwrap();
    let ask_chatml = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "--worker-backend",
            "deterministic",
            "ask",
            "--model",
            "qwen3-chatml",
            "--scope",
            ws.to_str().unwrap(),
            "--prompt",
            "Read file:guide.md and report option",
            "--approve-once",
        ])
        .output()
        .unwrap();
    assert!(ask_chatml.status.success());
    let ask_stdout = String::from_utf8_lossy(&ask_chatml.stdout);
    assert!(ask_stdout.contains("local_safe_mode"));
    assert!(ask_stdout.contains("guide.md"));

    // 8. Verify terminal ASCII art banner via `lora banner`
    let banner_out = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .env("NO_COLOR", "1")
        .args(["banner"])
        .output()
        .unwrap();
    assert!(banner_out.status.success());
    let banner_stdout = String::from_utf8_lossy(&banner_out.stdout);
    assert!(banner_stdout.contains("▄▒▀▀░"));
    assert!(banner_stdout.contains("Lora Agent CLI"));
}

#[test]
fn catalog_variant_validation_and_range_resume_etag() {
    let valid_sha = "a".repeat(64);
    let valid_variant = CatalogVariantDescriptor {
        model_id: "org/qwen-3b-instruct".to_string(),
        immutable_revision: "9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a1f0e".to_string(),
        filename: "qwen-3b-q4_k_m.gguf".to_string(),
        size_bytes: 1_800_000_000,
        license: "Apache-2.0".to_string(),
        expected_sha256: valid_sha.clone(),
        strong_etag: "\"etag-strong-9988\"".to_string(),
        download_url: "https://huggingface.co/org/qwen/resolve/9f8e7d/qwen-3b-q4_k_m.gguf"
            .to_string(),
    };
    assert!(validate_catalog_variant(&valid_variant, 1).is_ok());

    // 1. Mutable branch revision ('main') or excessive redirects rejected
    let mut mutable_branch = valid_variant.clone();
    mutable_branch.immutable_revision = "main".to_string();
    assert!(validate_catalog_variant(&mutable_branch, 1).is_err());
    assert!(validate_catalog_variant(&valid_variant, 5).is_err());

    // 2. Pickle / Python / archive artifacts rejected
    let mut pickle_variant = valid_variant.clone();
    pickle_variant.filename = "pytorch_model.bin".to_string();
    assert_eq!(
        validate_catalog_variant(&pickle_variant, 0)
            .unwrap_err()
            .reason_code(),
        ReasonCode::PolicyDenied
    );

    // 3. Range / If-Range resume appends only when status=206, strong ETag, revision, and offset match
    let ckpt = DownloadResumeCheckpoint {
        immutable_revision: valid_variant.immutable_revision.clone(),
        expected_sha256: valid_sha,
        strong_etag: "\"etag-strong-9988\"".to_string(),
        total_size_bytes: 1_800_000_000,
        bytes_received: 500_000_000,
    };
    assert_eq!(
        evaluate_range_resume(
            &ckpt,
            206,
            "\"etag-strong-9988\"",
            &valid_variant.immutable_revision,
            Some(500_000_000),
        )
        .unwrap(),
        ResumeDecision::AppendToPartial
    );

    // Changed ETag or weak ETag discards partial file and restarts
    assert_eq!(
        evaluate_range_resume(
            &ckpt,
            206,
            "\"etag-changed\"",
            &valid_variant.immutable_revision,
            Some(500_000_000),
        )
        .unwrap(),
        ResumeDecision::DiscardPartialAndRestart
    );
    assert_eq!(
        evaluate_range_resume(
            &ckpt,
            206,
            "W/\"weak-etag\"",
            &valid_variant.immutable_revision,
            Some(500_000_000),
        )
        .unwrap(),
        ResumeDecision::DiscardPartialAndRestart
    );

    // 4. Provider retry policy: never fall back from local failure to cloud egress, and require explicit user retry after streaming started
    assert_eq!(
        evaluate_provider_retry_policy(ProviderFailureStage::LocalInferenceFailed, 0, true),
        ProviderRetryDecision::ForbiddenLocalToCloudFallback
    );
    assert_eq!(
        evaluate_provider_retry_policy(ProviderFailureStage::PreSendConnection, 1, false),
        ProviderRetryDecision::RetryWithBoundedJitter
    );
    assert_eq!(
        evaluate_provider_retry_policy(
            ProviderFailureStage::PostSubmitOrStreamingStarted,
            1,
            false
        ),
        ProviderRetryDecision::RequireExplicitUserRetry
    );
}

#[test]
fn broker_permit_replay_reconciliation_and_compatibility_matrix() {
    use lora_protocol::ProbedModelMetadata;

    // 1. Artifact compatibility matrix (`docs/INFERENCE_ENGINE.md`)
    assert!(classify_artifact_compatibility("qwen2.5-3b-instruct-q4_k_m.gguf").is_ok());
    assert_eq!(
        classify_artifact_compatibility("model.safetensors")
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );
    assert_eq!(
        classify_artifact_compatibility("vision-mmproj-f16.gguf")
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );
    assert_eq!(
        classify_artifact_compatibility("llama-00001-of-00003.gguf")
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );
    assert_eq!(
        classify_artifact_compatibility("custom-lora-adapter.gguf")
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );

    // 2. Rank fitting alternatives locally before load
    let planner = AdmissionPlanner::new(DEFAULT_SAFETY_RESERVE_BYTES);
    let snap = HostSnapshot {
        os_name: "windows".to_string(),
        cpu_arch: "x86_64".to_string(),
        cpu_brand: "TestCPU".to_string(),
        physical_cores: 8,
        logical_cores: 16,
        cpu_isa_features: vec!["avx2".to_string()],
        total_ram_bytes: 2 * 1024 * 1024 * 1024,
        available_ram_bytes: 600 * 1024 * 1024, // 600 MiB available
        free_disk_bytes: 10 * 1024 * 1024 * 1024,
        charging_state: ChargingState::AcOnline,
        sample_timestamp_ms: 1000,
        unavailable_sensors: vec![],
    };
    let small_meta = ProbedModelMetadata {
        format: "gguf".to_string(),
        architecture: "llama".to_string(),
        model_name: "small-1b".to_string(),
        quantization: "Q4_K_M".to_string(),
        license: "Apache-2.0".to_string(),
        context_length: 4096,
        block_count: 16,
        head_count: 8,
        kv_head_count: 4,
        embedding_length: 512,
        key_dim: 64,
        value_dim: 64,
        kv_elem_bytes: 2,
        has_chat_template: true,
        template_digest: "abc".to_string(),
        supports_tool_calling: true,
        tensor_count: 32,
        total_tensor_bytes: 200 * 1024 * 1024,
        file_size_bytes: 200 * 1024 * 1024,
    };
    let mut huge_meta = small_meta.clone();
    huge_meta.total_tensor_bytes = 1200 * 1024 * 1024;
    huge_meta.file_size_bytes = 1200 * 1024 * 1024;

    let ranked = planner.rank_fitting_alternatives(
        &[
            ("huge-7b".to_string(), huge_meta),
            ("small-1b".to_string(), small_meta),
        ],
        2048,
        &snap,
    );
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].display_alias, "small-1b");

    // 3. Privileged broker single-use permit, replay rejection, and indeterminate reconciliation (`docs/OS_INTEGRATION.md`)
    let mut broker = PrivilegedBrokerGate::new(vec!["lora-local-service".to_string()]);
    let verb = BrokerVerb::RestartAllowlistedService {
        service_name: "lora-local-service".to_string(),
    };
    let permit = BrokerSingleUsePermit {
        permit_id: "p-1".to_string(),
        session_id: "sess-1".to_string(),
        challenge_nonce: "nonce-unique-001".to_string(),
        verb: verb.clone(),
        request_digest: BrokerSingleUsePermit::compute_request_digest(
            "sess-1",
            "nonce-unique-001",
            &verb,
        ),
        expires_at_ms: 50_000,
    };

    assert_eq!(
        broker.validate_and_consume_permit(&permit, 10_000).unwrap(),
        BrokerStatus::Completed
    );
    // Replaying the same permit fails closed
    assert!(broker.validate_and_consume_permit(&permit, 10_000).is_err());

    // After an indeterminate disconnection, repeating a service operation returns UnknownOutcomeNeedsReconciliation until reconciled
    broker.mark_indeterminate_disconnection("lora-local-service", "p-2");
    let permit2 = BrokerSingleUsePermit {
        permit_id: "p-3".to_string(),
        session_id: "sess-1".to_string(),
        challenge_nonce: "nonce-unique-003".to_string(),
        verb: verb.clone(),
        request_digest: BrokerSingleUsePermit::compute_request_digest(
            "sess-1",
            "nonce-unique-003",
            &verb,
        ),
        expires_at_ms: 50_000,
    };
    assert_eq!(
        broker
            .validate_and_consume_permit(&permit2, 10_000)
            .unwrap(),
        BrokerStatus::UnknownOutcomeNeedsReconciliation
    );
    broker.reconcile_resource("lora-local-service");
    assert_eq!(
        broker
            .validate_and_consume_permit(&permit2, 10_000)
            .unwrap(),
        BrokerStatus::Completed
    );
}

#[test]
fn audit_retention_redacted_export_and_incident_recovery() {
    use lora_core::{
        ApprovalDecision, ApprovalPromptPort, ExecutionMode, LoraApplicationService, RunState,
    };
    use lora_platform::{InProcessWorkerSpawner, OsStoreLockPort, SysinfoHardwareProbe};
    use std::sync::Arc;

    struct DenyPrompt;
    impl ApprovalPromptPort for DenyPrompt {
        fn prompt_user(&self, _req: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
            Ok(ApprovalDecision::Deny)
        }
    }

    // 1. Keyed correlation digest is deterministic per install key and differs across installs
    let d1 = keyed_correlation_digest("install-key-alpha", "user-identifier-42");
    let d2 = keyed_correlation_digest("install-key-alpha", "user-identifier-42");
    let d3 = keyed_correlation_digest("install-key-beta", "user-identifier-42");
    assert_eq!(d1, d2);
    assert_ne!(d1, d3);

    // 2. Audit export redacts planted secrets (`R17`) and retention pruning bounds stored events
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("sec_data");
    let lock_port = OsStoreLockPort;
    let app = LoraApplicationService::open(
        &data_dir,
        DEFAULT_SAFETY_RESERVE_BYTES,
        &lock_port,
        Arc::new(SysinfoHardwareProbe),
        Arc::new(OsScopedFileExecutor),
        Arc::new(InProcessWorkerSpawner::default()),
        Arc::new(DenyPrompt),
    )
    .unwrap();

    for i in 1..=5 {
        app.store
            .append_audit(
                &format!("run-{i}"),
                "mina",
                1,
                "dig",
                "guide.md",
                "allow",
                "ok",
                &format!("event {i} with CANARY_SECRET_LEAK_99"),
            )
            .unwrap();
    }

    let exported = app.store.export_redacted_audit_bundle(None).unwrap();
    assert!(!exported.contains("CANARY_SECRET_LEAK_99"));
    assert!(exported.contains("[REDACTED]"));

    let pruned = app.store.prune_audit_retention(2).unwrap();
    assert_eq!(pruned, 3);
    assert_eq!(app.store.list_audit_events(None).unwrap().len(), 2);

    // 3. Emergency incident recovery revokes grants, quarantines target extension, and interrupts active runs
    let mut host = app.store.load_extension_host().unwrap();
    let manifest = ExtensionManifestV1 {
        manifest_version: 1,
        id: "org.example.suspect".to_string(),
        version: "1.0.0".to_string(),
        kind: "wasm-component".to_string(),
        host_api: HostApiVersion {
            major: 1,
            minimum_minor: 0,
        },
        entry: "mod.wasm".to_string(),
        exports: vec!["tool.v1".to_string()],
        requests: vec![],
        dependencies: vec![],
    };
    host.stage_and_verify(manifest, "sha_sus", "sha_sus")
        .unwrap();
    host.activate("org.example.suspect", &[]).unwrap();
    app.store.save_extension_host(&host).unwrap();

    app.store
        .create_run(
            "run-in-flight-99",
            "mina",
            "ws-1",
            ExecutionMode::ChatOnly.as_str(),
            "sha_model",
        )
        .unwrap();

    let receipt = app
        .execute_incident_recovery(Some("org.example.suspect"))
        .unwrap();
    assert!(receipt.new_revocation_epoch >= 1);
    assert_eq!(
        receipt.quarantined_extension.as_deref(),
        Some("org.example.suspect")
    );
    assert!(receipt
        .interrupted_runs
        .contains(&"run-in-flight-99".to_string()));

    let post_host = app.store.load_extension_host().unwrap();
    assert_eq!(
        post_host.get("org.example.suspect").unwrap().state,
        ExtensionState::Quarantined
    );
    let (state, _, _) = app
        .store
        .get_run_state("run-in-flight-99")
        .unwrap()
        .unwrap();
    assert_eq!(state, RunState::Interrupted);
}

#[test]
fn performance_stage_latencies_unified_memory_and_report_anonymization() {
    // 1. Human-visible TTFT includes queue + UI render delivery; engine decode excludes UI delivery
    let profile = StageLatencyProfile {
        request_id: "req-perf-1".to_string(),
        queue_us: 100,
        handshake_us: 200,
        admission_us: 150,
        load_us: 1000,
        prefill_to_first_byte_us: 500,
        ui_render_delivery_us: 50,
        decode_last_token_us: 4000,
        reap_us: 120,
    };
    assert_eq!(profile.human_visible_ttft_us(), 2000);
    assert_eq!(profile.engine_decode_duration_us(), 4000);

    // 2. Unified memory avoids summing RAM + VRAM as independent physical pools (`R02`)
    let unified = compute_unified_memory_budget(8_000_000_000, Some(6_000_000_000), true);
    assert_eq!(unified, 6_000_000_000);
    let discrete = compute_unified_memory_budget(8_000_000_000, Some(6_000_000_000), false);
    assert_eq!(discrete, 14_000_000_000);

    // 3. Public benchmark report anonymization redacts paths, usernames, hostnames, and serials
    let raw = "host=HOSTNAME=workstation-01 path=C:\\Users\\alice\\models\\private.gguf serial=SERIAL=SN998877";
    let anon = anonymize_public_benchmark_report(raw);
    assert!(!anon.contains("alice"));
    assert!(!anon.contains("workstation-01"));
    assert!(!anon.contains("SN998877"));
    assert!(anon.contains("[ANONYMIZED]"));
}

#[test]
fn rag_context_budgeting_rrf_namespace_and_specialist_handoff() {
    use lora_core::{
        allocate_context_budget, reciprocal_rank_fusion, validate_specialist_handoff,
        ContextBudgetRequest, EmbeddingNamespace, RetrievedEvidence, SpecialistHandoffEnvelope,
    };

    // 1. Embedding namespace compatibility rejects dimension, model digest, or revision mismatch (`docs/MEMORY_AND_RAG.md`)
    let ns_index = EmbeddingNamespace {
        model_digest: "sha256:embed-nomic-v1.5".to_string(),
        tokenizer: "bert-wordpiece".to_string(),
        dimension: 768,
        normalized: true,
        chunker_revision: 1,
        parser_revision: 1,
    };
    assert!(ns_index.verify_compatible_namespace(&ns_index).is_ok());

    let mut ns_wrong_dim = ns_index.clone();
    ns_wrong_dim.dimension = 384;
    assert_eq!(
        ns_index
            .verify_compatible_namespace(&ns_wrong_dim)
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );

    let mut ns_wrong_model = ns_index.clone();
    ns_wrong_model.model_digest = "sha256:embed-bge-small".to_string();
    assert_eq!(
        ns_index
            .verify_compatible_namespace(&ns_wrong_model)
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );

    // 2. Reciprocal-Rank Fusion (`RRF`) promotes chunks present in both lexical and vector rankings
    let ev_a = RetrievedEvidence {
        chunk_id: "chk-a".to_string(),
        workspace_id: "ws-1".to_string(),
        relative_path: "arch.md".to_string(),
        doc_sha256: "sha_a".to_string(),
        byte_range: (0, 100),
        line_range: (1, 10),
        snippet: "architecture overview".to_string(),
        rank_score: 1,
    };
    let ev_b = RetrievedEvidence {
        chunk_id: "chk-b".to_string(),
        workspace_id: "ws-1".to_string(),
        relative_path: "policy.md".to_string(),
        doc_sha256: "sha_b".to_string(),
        byte_range: (0, 120),
        line_range: (1, 12),
        snippet: "policy gate rules".to_string(),
        rank_score: 1,
    };
    let ev_c = RetrievedEvidence {
        chunk_id: "chk-c".to_string(),
        workspace_id: "ws-1".to_string(),
        relative_path: "store.md".to_string(),
        doc_sha256: "sha_c".to_string(),
        byte_range: (0, 90),
        line_range: (1, 8),
        snippet: "sqlite wal store".to_string(),
        rank_score: 1,
    };

    // chk-b appears in both lists (rank 2 in lexical, rank 1 in vector) -> highest fused RRF score
    let fused = reciprocal_rank_fusion(
        &[ev_a.clone(), ev_b.clone()],
        &[ev_b.clone(), ev_c.clone()],
        60,
        3,
    );
    assert_eq!(fused.len(), 3);
    assert_eq!(fused[0].chunk_id, "chk-b");

    // 3. Context budgeting reserves output + schema overhead first, protects user request, and drops lower-ranked excerpts (`docs/MEMORY_AND_RAG.md`)
    let budget_req = ContextBudgetRequest {
        context_window_tokens: 1000,
        reserved_output_tokens: 250,
        control_schema_overhead_tokens: 100,
        user_request_tokens: 200,
        recent_turns_tokens: 150,
        tool_receipts_tokens: 100,
        ranked_excerpts: vec![(ev_b.clone(), 120), (ev_a.clone(), 150), (ev_c.clone(), 50)],
    };
    // Remaining after mandatory(350) + user(200) + receipts(100) + turns(150) = 200 tokens.
    // Admits ev_b (120 tokens, leaving 80), drops ev_a (150 > 80), admits ev_c (50 <= 80, leaving 30).
    let alloc = allocate_context_budget(&budget_req).unwrap();
    assert_eq!(alloc.admitted_excerpts.len(), 2);
    assert_eq!(alloc.admitted_excerpts[0].chunk_id, "chk-b");
    assert_eq!(alloc.admitted_excerpts[1].chunk_id, "chk-c");
    assert_eq!(alloc.dropped_excerpt_ids, vec!["chk-a".to_string()]);
    assert_eq!(alloc.remaining_headroom_tokens, 30);

    // If user request + reserved overhead exceeds the context window, fail closed without silent truncation
    let mut oversized_user_req = budget_req.clone();
    oversized_user_req.user_request_tokens = 800;
    assert_eq!(
        allocate_context_budget(&oversized_user_req)
            .unwrap_err()
            .reason_code(),
        ReasonCode::BudgetExhausted
    );

    // 4. Specialist sub-agent handoff validation (`docs/AGENT_RUNTIME.md`): bounded depth, cycle detection, and strict authority subset
    let valid_handoff = SpecialistHandoffEnvelope {
        parent_run_id: "run-parent-1".to_string(),
        child_run_id: "run-child-2".to_string(),
        objective: "Verify citations in docs/".to_string(),
        result_schema: "CitationAuditV1".to_string(),
        depth: 1,
        max_depth: 2,
        remaining_tool_calls: 2,
        remaining_tokens: 512,
        parent_allowed_roots: vec!["ws-docs".to_string(), "ws-src".to_string()],
        requested_allowed_roots: vec!["ws-docs".to_string()],
        visited_run_ids: vec!["run-parent-1".to_string()],
    };
    assert!(validate_specialist_handoff(&valid_handoff).is_ok());

    // Authority escalation rejected
    let mut escalated = valid_handoff.clone();
    escalated.requested_allowed_roots = vec!["ws-docs".to_string(), "ws-secrets".to_string()];
    assert_eq!(
        validate_specialist_handoff(&escalated)
            .unwrap_err()
            .reason_code(),
        ReasonCode::ScopeEscapeAttempt
    );

    // Cycle or depth overflow rejected
    let mut cycled = valid_handoff.clone();
    cycled.visited_run_ids.push("run-child-2".to_string());
    assert_eq!(
        validate_specialist_handoff(&cycled)
            .unwrap_err()
            .reason_code(),
        ReasonCode::BudgetExhausted
    );

    // 5. Controlled benchmark comparison verifier (`docs/BENCHMARK_PLAN.md`) and Risk Register matrix (`R01`..`R18`)
    use lora_core::{
        default_risk_register_matrix, verify_benchmark_comparison_controls,
        BenchmarkComparisonControl,
    };
    let base_ctrl = BenchmarkComparisonControl {
        model_sha256: "sha_qwen_3b".to_string(),
        quantization: "Q4_K_M".to_string(),
        tokenizer_digest: "tok_v1".to_string(),
        template_digest: "tpl_v1".to_string(),
        effective_context_length: 2048,
        prompt_token_ids: vec![101, 202, 303, 404],
    };
    assert!(verify_benchmark_comparison_controls(&base_ctrl, &base_ctrl).is_ok());

    let mut diff_tokens = base_ctrl.clone();
    diff_tokens.prompt_token_ids = vec![101, 202, 999];
    assert_eq!(
        verify_benchmark_comparison_controls(&base_ctrl, &diff_tokens)
            .unwrap_err()
            .reason_code(),
        ReasonCode::PolicyDenied
    );

    let mut diff_quant = base_ctrl.clone();
    diff_quant.quantization = "Q8_0".to_string();
    assert_eq!(
        verify_benchmark_comparison_controls(&base_ctrl, &diff_quant)
            .unwrap_err()
            .reason_code(),
        ReasonCode::IncompatibleModel
    );

    let risks = default_risk_register_matrix();
    assert_eq!(risks.len(), 18);
    assert!(risks.iter().all(|r| r.verified));
    assert_eq!(risks.first().unwrap().risk_id, "R01");
    assert_eq!(risks.last().unwrap().risk_id, "R18");
}

/// `v0.1.2`: RAG-grounded `lora ask --use-rag` and persisted multi-turn `lora chat --history-file`.
#[test]
fn cli_rag_grounded_ask_and_multi_turn_chat_history() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("rag_data");
    let fixture = tmp.path().join("rag_fixture.gguf");
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    fs::write(
        ws.join("deploy.md"),
        "Deployment notes for the lattice rollout\nCONFIG_VALUE=amber_lattice_42\n",
    )
    .unwrap();

    let run = |args: &[&str]| {
        Command::new(lora_bin)
            .env("LORA_DATA_DIR", &data_dir)
            .args(args)
            .output()
            .unwrap()
    };

    assert!(run(&["generate-fixture", fixture.to_str().unwrap()])
        .status
        .success());
    assert!(
        run(&["import", fixture.to_str().unwrap(), "--alias", "rag-model"])
            .status
            .success()
    );
    assert!(run(&[
        "index",
        "--scope",
        ws.to_str().unwrap(),
        "--files",
        "deploy.md"
    ])
    .status
    .success());

    // 1. RAG-grounded ask answers from retrieved FTS5 evidence even with tool calls denied
    let ask = run(&[
        "ask",
        "--model",
        "rag-model",
        "--scope",
        ws.to_str().unwrap(),
        "--prompt",
        "What is the lattice deployment CONFIG_VALUE AND rollout?",
        "--deny",
        "--use-rag",
    ]);
    let ask_stdout = String::from_utf8_lossy(&ask.stdout);
    assert!(
        ask.status.success(),
        "ask --use-rag failed: {}",
        String::from_utf8_lossy(&ask.stderr)
    );
    assert!(ask_stdout.contains("amber_lattice_42"), "{ask_stdout}");
    assert!(ask_stdout.contains("Citation: deploy.md"), "{ask_stdout}");

    // 2. Chat history is persisted and prior turns are admitted on the next turn
    let history = tmp.path().join("history.json");
    let hist = history.to_str().unwrap();
    let first = run(&[
        "chat",
        "--model",
        "rag-model",
        "--prompt",
        "First turn",
        "--history-file",
        hist,
    ]);
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stdout).contains("[History: 0/0"));
    let second = run(&[
        "chat",
        "--model",
        "rag-model",
        "--prompt",
        "Second turn",
        "--history-file",
        hist,
    ]);
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("[History: 2/2"));
    let turns: Vec<lora_core::ChatTurn> =
        serde_json::from_str(&fs::read_to_string(&history).unwrap()).unwrap();
    assert_eq!(turns.len(), 4);
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[0].content, "First turn");
    assert_eq!(turns[1].role, "assistant");

    // 3. Interactive mode reads prompts from stdin until `/exit`
    let mut child = Command::new(lora_bin)
        .env("LORA_DATA_DIR", &data_dir)
        .args([
            "chat",
            "--model",
            "rag-model",
            "--interactive",
            "--history-file",
            hist,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Third turn\n\n/exit\nignored\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("[History: 4/4"));
    let turns: Vec<lora_core::ChatTurn> =
        serde_json::from_str(&fs::read_to_string(&history).unwrap()).unwrap();
    assert_eq!(turns.len(), 6);

    // 4. Malformed history files fail closed rather than being silently overwritten
    fs::write(&history, "{not json").unwrap();
    let bad = run(&[
        "chat",
        "--model",
        "rag-model",
        "--prompt",
        "x",
        "--history-file",
        hist,
    ]);
    assert!(!bad.status.success());
    assert_eq!(fs::read_to_string(&history).unwrap(), "{not json");
}

/// `v0.1.4`: Supervised Agent patch apply with CompensatingPatchReceipt and rollback in CLI.
#[test]
fn cli_patch_apply_with_receipt_and_rollback() {
    use std::process::Command;

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    let file_path = ws.join("config.toml");
    fs::write(&file_path, "timeout = 30\nretries = 3\n").unwrap();

    let initial_bytes = fs::read(&file_path).unwrap();
    let initial_sha = format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(&initial_bytes)
    );
    let receipt_path = tmp.path().join("patch_receipt.json");

    // 1. Apply patch and save receipt JSON
    let patch_out = Command::new(lora_bin)
        .args([
            "patch",
            "--scope",
            ws.to_str().unwrap(),
            "--file",
            "config.toml",
            "--expected-sha256",
            &initial_sha,
            "--old-snippet",
            "timeout = 30",
            "--new-snippet",
            "timeout = 60",
            "--apply",
            "--receipt-out",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        patch_out.status.success(),
        "{}",
        String::from_utf8_lossy(&patch_out.stderr)
    );
    assert!(receipt_path.exists());
    assert_eq!(
        fs::read_to_string(&file_path).unwrap(),
        "timeout = 60\nretries = 3\n"
    );

    // 2. Rollback using the receipt JSON restores the original file
    let rollback_out = Command::new(lora_bin)
        .args([
            "rollback",
            "--scope",
            ws.to_str().unwrap(),
            "--receipt",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rollback_out.status.success(),
        "{}",
        String::from_utf8_lossy(&rollback_out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&file_path).unwrap(),
        "timeout = 30\nretries = 3\n"
    );
}

#[test]
fn cli_plugins_stage_activate_disable_and_list() {
    use std::process::Command;

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let manifest_path = tmp.path().join("manifest.json");

    let manifest = serde_json::json!({
        "manifest_version": 1,
        "id": "org.cli.formatter",
        "version": "1.0.0",
        "kind": "wasm-component",
        "host_api": { "major": 1, "minimum_minor": 0 },
        "entry": "plugin.wasm",
        "exports": ["tool.format.v1"],
        "requests": ["fs.read"],
        "dependencies": []
    });
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let run = |args: &[&str]| {
        Command::new(lora_bin)
            .env("LORA_DATA_DIR", &data_dir)
            .args(args)
            .output()
            .unwrap()
    };

    // 1. Stage manifest
    let out_stage = run(&[
        "plugins",
        "--stage-manifest",
        manifest_path.to_str().unwrap(),
        "--sha256",
        "sha256_mock_hash",
    ]);
    assert!(
        out_stage.status.success(),
        "{}",
        String::from_utf8_lossy(&out_stage.stderr)
    );
    let stdout_stage = String::from_utf8_lossy(&out_stage.stdout);
    assert!(stdout_stage.contains("Staged and verified extension 'org.cli.formatter'"));

    // 2. Activate with required grant
    let out_act = run(&[
        "plugins",
        "--activate",
        "org.cli.formatter",
        "--grant",
        "fs.read",
    ]);
    assert!(
        out_act.status.success(),
        "{}",
        String::from_utf8_lossy(&out_act.stderr)
    );
    let stdout_act = String::from_utf8_lossy(&out_act.stdout);
    assert!(stdout_act.contains("Activated extension 'org.cli.formatter' -> Active"));

    // 3. Disable and revoke permissions
    let out_dis = run(&["plugins", "--disable", "org.cli.formatter"]);
    assert!(
        out_dis.status.success(),
        "{}",
        String::from_utf8_lossy(&out_dis.stderr)
    );
    let stdout_dis = String::from_utf8_lossy(&out_dis.stdout);
    assert!(stdout_dis.contains("Disabled extension 'org.cli.formatter' -> Disabled"));

    // 4. List plugins
    let out_list = run(&["plugins"]);
    assert!(
        out_list.status.success(),
        "{}",
        String::from_utf8_lossy(&out_list.stderr)
    );
    let stdout_list = String::from_utf8_lossy(&out_list.stdout);
    assert!(stdout_list.contains("org.cli.formatter v1.0.0 [Disabled]"));
}

#[test]
fn cli_tune_calibration_and_list() {
    use std::process::Command;

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let fixture = tmp.path().join("tune_fixture.gguf");
    let receipt_path = tmp.path().join("tune_receipt.json");

    let run = |args: &[&str]| {
        Command::new(lora_bin)
            .env("LORA_DATA_DIR", &data_dir)
            .args(args)
            .output()
            .unwrap()
    };

    assert!(run(&["generate-fixture", fixture.to_str().unwrap()])
        .status
        .success());
    assert!(
        run(&["import", fixture.to_str().unwrap(), "--alias", "tune-model"])
            .status
            .success()
    );

    // 1. Run calibration and export receipt
    let out_tune = run(&[
        "tune",
        "--model",
        "tune-model",
        "--context",
        "2048",
        "--receipt-out",
        receipt_path.to_str().unwrap(),
    ]);
    assert!(
        out_tune.status.success(),
        "{}",
        String::from_utf8_lossy(&out_tune.stderr)
    );
    let stdout_tune = String::from_utf8_lossy(&out_tune.stdout);
    assert!(stdout_tune.contains("Optimal threads"));
    assert!(stdout_tune.contains("Optimal batch size"));
    assert!(receipt_path.exists());

    // 2. List stored tuning receipts
    let out_list = run(&["tune", "--list"]);
    assert!(
        out_list.status.success(),
        "{}",
        String::from_utf8_lossy(&out_list.stderr)
    );
    let stdout_list = String::from_utf8_lossy(&out_list.stdout);
    assert!(stdout_list.contains("Stored tuning receipts: 1"));
}

#[test]
fn cli_adapter_dataset_split_and_export() {
    use std::process::Command;

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let dataset_path = tmp.path().join("dataset.json");
    let out_dir = tmp.path().join("splits_out");

    let examples = serde_json::json!([
        {
            "example_id": "ex1",
            "source_group_id": "doc_alpha",
            "prompt": "Prompt Alpha",
            "target_completion": "Completion Alpha"
        },
        {
            "example_id": "ex2",
            "source_group_id": "doc_beta",
            "prompt": "Prompt Beta",
            "target_completion": "Completion Beta"
        },
        {
            "example_id": "ex3",
            "source_group_id": "doc_gamma",
            "prompt": "Prompt Gamma",
            "target_completion": "Completion Gamma"
        }
    ]);
    fs::write(
        &dataset_path,
        serde_json::to_string_pretty(&examples).unwrap(),
    )
    .unwrap();

    let run = |args: &[&str]| {
        Command::new(lora_bin)
            .env("LORA_DATA_DIR", &data_dir)
            .args(args)
            .output()
            .unwrap()
    };

    let out = run(&[
        "adapter",
        "--dataset",
        dataset_path.to_str().unwrap(),
        "--license",
        "Apache-2.0",
        "--out-dir",
        out_dir.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Dataset SHA-256"));
    assert!(stdout.contains("Splits (tr/va/te): 1/1/1 examples"));
    assert!(out_dir.join("manifest.json").exists());
    assert!(out_dir.join("train.json").exists());
    assert!(out_dir.join("val.json").exists());
    assert!(out_dir.join("test.json").exists());
}

#[test]
fn cli_audit_journal_export_prune_and_filter() {
    use std::process::Command;

    let lora_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let export_path = tmp.path().join("audit_export.json");

    let run = |args: &[&str]| {
        Command::new(lora_bin)
            .env("LORA_DATA_DIR", &data_dir)
            .args(args)
            .output()
            .unwrap()
    };

    // 1. Initially empty audit journal
    let out_init = run(&["audit"]);
    assert!(out_init.status.success());

    // 2. Execute incident recovery which logs an audit event
    let out_rec = run(&["incident-recover"]);
    assert!(out_rec.status.success());

    // 3. Inspect audit events
    let out_list = run(&["audit"]);
    assert!(out_list.status.success());
    let stdout_list = String::from_utf8_lossy(&out_list.stdout);
    assert!(stdout_list.contains("security-coordinator"));
    assert!(stdout_list.contains("incident-recovery"));
    assert!(stdout_list.contains("incident-recovery"));

    // 4. Export redacted audit bundle
    let out_exp = run(&["audit", "--export", export_path.to_str().unwrap()]);
    assert!(out_exp.status.success());
    assert!(export_path.exists());
    let exported_data = fs::read_to_string(&export_path).unwrap();
    assert!(exported_data.starts_with('['));

    // 5. Prune audit retention
    let out_prune = run(&["audit", "--prune-keep", "10"]);
    assert!(out_prune.status.success());
    let stdout_prune = String::from_utf8_lossy(&out_prune.stdout);
    assert!(stdout_prune.contains("Pruned"));
}

#[test]
fn cli_catalog_context_budget_and_broker_execution() {
    use std::fs;
    use std::process::Command;
    let cli_bin = env!("CARGO_BIN_EXE_lora");
    let tmp = TempDir::new().expect("tempdir");

    let run = |args: &[&str]| {
        Command::new(cli_bin)
            .env("LORA_DATA_DIR", tmp.path())
            .args(args)
            .output()
            .expect("run lora CLI")
    };

    // 1. Catalog variant verification via CLI
    let valid_desc = serde_json::json!({
        "model_id": "org/test-model",
        "immutable_revision": "fedcba9876543210fedcba9876543210fedcba98",
        "filename": "model-q4_k_m.gguf",
        "size_bytes": 1048576,
        "license": "Apache-2.0",
        "expected_sha256": "a".repeat(64),
        "strong_etag": "\"etag-12345\"",
        "download_url": "https://cdn.example.com/models/model-q4_k_m.gguf"
    });
    let desc_path = tmp.path().join("catalog-desc.json");
    fs::write(&desc_path, valid_desc.to_string()).unwrap();

    let out_cat = run(&["catalog", "--descriptor", desc_path.to_str().unwrap()]);
    assert!(out_cat.status.success());
    let stdout_cat = String::from_utf8_lossy(&out_cat.stdout);
    assert!(stdout_cat.contains("Catalog variant verified"));
    assert!(stdout_cat.contains("model-q4_k_m.gguf"));

    // 2. Context budget evaluation via CLI
    let out_budget = run(&[
        "context-budget",
        "--window",
        "2048",
        "--output",
        "512",
        "--user",
        "256",
    ]);
    assert!(out_budget.status.success());
    let stdout_budget = String::from_utf8_lossy(&out_budget.stdout);
    assert!(stdout_budget.contains("Context window     : 2048 tokens"));
    assert!(stdout_budget.contains("Reserved output    : 512 tokens"));
    assert!(stdout_budget.contains("User request       : 256 tokens"));

    // 3. Privileged broker permit validation via CLI
    let out_broker = run(&[
        "broker",
        "--service",
        "lora-local-service",
        "--session",
        "test-session-1",
        "--nonce",
        "nonce-test-123",
    ]);
    assert!(out_broker.status.success());
    let stdout_broker = String::from_utf8_lossy(&out_broker.stdout);
    assert!(stdout_broker.contains("Broker service : lora-local-service"));
    assert!(stdout_broker.contains("Status         : Completed"));

    // 4. Model format compatibility matrix classification via CLI
    let out_compat = run(&["compatibility", "--filename", "qwen-3b-q4_k_m.gguf"]);
    assert!(out_compat.status.success());
    let stdout_compat = String::from_utf8_lossy(&out_compat.stdout);
    assert!(stdout_compat.contains("Artifact   : qwen-3b-q4_k_m.gguf"));
    assert!(stdout_compat.contains("Supported  : true"));

    // 5. Specialist sub-agent delegation handoff verification via CLI
    let out_handoff = run(&[
        "handoff-check",
        "--parent-run",
        "run-parent-42",
        "--child-run",
        "run-child-43",
        "--depth",
        "1",
        "--max-depth",
        "2",
    ]);
    assert!(out_handoff.status.success());
    let stdout_handoff = String::from_utf8_lossy(&out_handoff.stdout);
    assert!(stdout_handoff.contains("Specialist handoff verified: run-parent-42 -> run-child-43"));
}

#[test]
fn workflow_pipeline_execution_and_step_accounting() {
    use lora_core::{execute_workflow_job, WorkflowJobConfig, WorkflowStepKind};

    let cfg = WorkflowJobConfig {
        workflow_id: "wf-pipeline-01".to_string(),
        title: "E2E Workflow Test".to_string(),
        steps: vec![
            WorkflowStepKind::IndexWorkspace {
                workspace_id: "ws-test".to_string(),
                relative_path: "docs/ARCHITECTURE.md".to_string(),
            },
            WorkflowStepKind::GroundedQuery {
                query: "Summarize architecture".to_string(),
                model: "qwen2.5".to_string(),
                context_tokens: 2048,
            },
            WorkflowStepKind::PreviewPatch {
                target_file: "docs/TEST.md".to_string(),
                replacement: "new text".to_string(),
            },
            WorkflowStepKind::AuditExport {
                output_path: "audit.json".to_string(),
            },
        ],
        stop_on_failure: true,
    };

    let receipt = execute_workflow_job(&cfg).expect("execute workflow");
    assert_eq!(receipt.total_steps, 4);
    assert_eq!(receipt.completed_steps, 4);
    assert!(receipt.successful);
    assert_eq!(receipt.step_outcomes.len(), 4);
    assert!(receipt.step_outcomes[0].success);
    assert!(receipt.step_outcomes[1].success);
    assert!(receipt.step_outcomes[2].success);
    assert!(receipt.step_outcomes[3].success);
}
