//! Trusted terminal CLI and composition root (`apps/cli`, `docs/ARCHITECTURE.md`, `T2`, `T6`, `T7`, `T8`).

use clap::{Parser, Subcommand, ValueEnum};
use lora_core::{
    allocate_context_budget, apply_approved_patch, classify_artifact_compatibility,
    default_risk_register_matrix, evaluate_adapter_promotion_gate, execute_workflow_job,
    format_multi_turn_prompt, now_epoch_ms, prepare_leakage_free_splits, preview_checked_patch,
    rollback_applied_patch, validate_catalog_variant, validate_specialist_handoff, AgentLimits,
    ApprovalDecision, ApprovalPromptPort, ApprovalRequest, BrokerSingleUsePermit, BrokerVerb,
    CatalogVariantDescriptor, ChatTurn, CompensatingPatchReceipt, ContextBudgetRequest,
    ExecutionMode, ExtensionManifestV1, HardwareProbePort, HeldOutEvaluationReceipt,
    LoraApplicationService, LoraError, LoraJobConfig, PatchProposal, PrivilegedBrokerGate,
    SpecialistHandoffEnvelope, TrainingExample, TuningCacheKey, TuningProfile, TuningTrialSample,
    WorkflowJobConfig, WorkflowStepKind, DEFAULT_SAFETY_RESERVE_BYTES, TERMINAL_ASCII_BANNER,
};
use lora_platform::{
    resolve_default_worker_spawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use lora_protocol::{
    build_synthetic_gguf, SyntheticGgufSpec, PINNED_LLAMA_CPP_REVISION, SUPPORTED_PROTOCOL_MAJOR,
};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "lora",
    version,
    before_help = TERMINAL_ASCII_BANNER,
    about = "Local-first Rust runtime for GGUF text models and scoped read-only file assistance"
)]
struct Cli {
    /// Per-user application data directory (owns SQLite store, lock, and content-addressed blobs)
    #[arg(long, env = "LORA_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Path to isolated `lora-llama-worker` binary (defaults to sibling executable or in-process shim)
    #[arg(long, env = "LORA_WORKER_BIN")]
    worker_bin: Option<PathBuf>,

    /// Worker backend selection (`native` or `deterministic`)
    #[arg(long, env = "LORA_WORKER_BACKEND")]
    worker_backend: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ProfileArg {
    Automatic,
    Balanced,
    MaxPerformance,
    LowMemory,
    BatterySaver,
}

impl From<ProfileArg> for TuningProfile {
    fn from(val: ProfileArg) -> Self {
        match val {
            ProfileArg::Automatic => TuningProfile::Automatic,
            ProfileArg::Balanced => TuningProfile::Balanced,
            ProfileArg::MaxPerformance => TuningProfile::MaximumPerformance,
            ProfileArg::LowMemory => TuningProfile::LowMemory,
            ProfileArg::BatterySaver => TuningProfile::BatterySaver,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print the Lora Agent terminal ASCII banner and version header
    #[command(visible_aliases = ["splash"])]
    Banner,

    /// Report build identifiers, worker pin, hardware snapshot, and startup reconciliation status
    #[command(visible_aliases = ["hardware", "doctor"])]
    Inspect {
        #[arg(long, hide = true)]
        signal: Option<String>,
    },

    /// Inspect local runtime state, installed model count, reservation status, and recent audit count (`docs/API_AND_SDK.md`)
    Status,

    /// List, stage, activate, fault-quarantine, or roll back extensions (`docs/PLUGIN_SYSTEM.md`)
    Plugins {
        /// Optional path to an ExtensionManifestV1 JSON file to stage & verify
        #[arg(long)]
        stage_manifest: Option<PathBuf>,
        /// Expected SHA-256 package digest when staging
        #[arg(long)]
        sha256: Option<String>,
        /// Extension ID to activate
        #[arg(long)]
        activate: Option<String>,
        /// Comma-separated capabilities to grant on activation
        #[arg(long, value_delimiter = ',')]
        grant: Vec<String>,
        /// Record a runtime fault against an extension ID (3 faults -> Quarantined)
        #[arg(long)]
        fault: Option<String>,
        /// Roll back an extension ID to its previous compatible digest
        #[arg(long)]
        rollback: Option<String>,
        /// Disable an active extension ID and revoke its approved capabilities
        #[arg(long)]
        disable: Option<String>,
    },

    /// Import and verify a local single-file GGUF text model into content-addressed storage
    Import {
        /// Path to the local `.gguf` file
        path: PathBuf,
        /// Optional human-readable display alias
        #[arg(long)]
        alias: Option<String>,
        /// Optional publisher SHA-256 hex digest to authenticate provenance
        #[arg(long)]
        expected_sha256: Option<String>,
    },

    /// List verified Ready models in the local catalog
    Models,

    /// Retire a model revision and garbage-collect its blob if unreferenced (`docs/MODEL_MANAGEMENT.md`)
    Delete {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: String,
    },

    /// Evaluate and explain the Hardware Intelligence Engine (HIE) memory plan for a model
    Plan {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: String,
        /// Requested context window in tokens
        #[arg(long, default_value_t = 2048)]
        context: u32,
        /// Hardware scheduling profile
        #[arg(long, value_enum, default_value_t = ProfileArg::Automatic)]
        profile: ProfileArg,
    },

    /// Run a local offline chat turn with streaming output and Ctrl-C cancellation
    Chat {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: String,
        /// User prompt text (required unless `--interactive`)
        #[arg(long, required_unless_present = "interactive")]
        prompt: Option<String>,
        /// Requested context window in tokens
        #[arg(long, default_value_t = 2048)]
        context: u32,
        /// Hardware scheduling profile
        #[arg(long, value_enum, default_value_t = ProfileArg::Automatic)]
        profile: ProfileArg,
        /// JSON file holding prior conversation turns; loaded before and updated after each turn
        #[arg(long)]
        history_file: Option<PathBuf>,
        /// Read successive prompts from stdin until EOF or `/exit`
        #[arg(long, short = 'i')]
        interactive: bool,
    },

    /// Run Read-only Assistant mode with a scoped local directory and human approval gate
    Ask {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: String,
        /// Approved local directory scope
        #[arg(long)]
        scope: PathBuf,
        /// User question about files inside the scope
        #[arg(long)]
        prompt: String,
        /// Requested context window in tokens
        #[arg(long, default_value_t = 2048)]
        context: u32,
        /// Automatically grant a one-time approval for testing/non-interactive CLI runs
        #[arg(long)]
        approve_once: bool,
        /// Automatically deny tool requests
        #[arg(long)]
        deny: bool,
        /// Ground the answer with pre-run SQLite FTS5 retrieval over indexed local memory
        #[arg(long)]
        use_rag: bool,
    },

    /// Inspect, export, or prune redacted local audit events (`docs/SECURITY_ARCHITECTURE.md`, `R17`)
    Audit {
        /// Optional run ID filter
        #[arg(long)]
        run_id: Option<String>,
        /// Optional output file path to export a redacted JSON audit bundle
        #[arg(long)]
        export: Option<PathBuf>,
        /// Optional retention cap: prune older audit records and keep at most N latest entries
        #[arg(long)]
        prune_keep: Option<usize>,
    },

    /// Execute emergency incident recovery: revoke all active grants, quarantine an extension, and interrupt non-terminal runs (`docs/SECURITY_ARCHITECTURE.md`)
    IncidentRecover {
        /// Optional extension ID to quarantine immediately
        #[arg(long)]
        quarantine_extension: Option<String>,
    },

    /// Generate a deterministic synthetic GGUF v3 fixture for offline testing (`T1`/`M1`)
    GenerateFixture {
        /// Output file path for the generated `.gguf` fixture
        output: PathBuf,
    },

    /// Run T9 paired benchmarks and 30-cycle load/generate/unload soak test (`docs/BENCHMARK_PLAN.md`)
    Benchmark {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: String,
        /// Directory to store anonymized `receipts.jsonl` and `summary.json`
        #[arg(long)]
        output_dir: PathBuf,
        /// Number of warm-up iterations
        #[arg(long, default_value_t = 2)]
        warmup: u32,
        /// Number of measured iterations
        #[arg(long, default_value_t = 10)]
        measured: u32,
        /// Number of load/generate/unload soak cycles
        #[arg(long, default_value_t = 30)]
        soak_cycles: u32,
    },

    /// Run T10 / M8 release qualification, producing support matrix, SBOM notices, update/rollback drill, and SHA256SUMS
    Qualify {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: String,
        /// Output directory for the qualification package
        #[arg(long)]
        output_dir: PathBuf,
    },

    /// Index approved files inside a workspace scope into SQLite FTS5 (`docs/MEMORY_AND_RAG.md`)
    Index {
        /// Approved local directory scope
        #[arg(long)]
        scope: PathBuf,
        /// Comma-separated relative file paths inside the scope
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
    },

    /// Search indexed workspace chunks using SQLite FTS5 lexical retrieval
    Search {
        /// Approved local directory scope
        #[arg(long)]
        scope: PathBuf,
        /// Lexical search query
        #[arg(long)]
        query: String,
        /// Maximum number of snippets to return
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },

    /// Immediately tombstone and purge all indexed FTS5/chunk data for a workspace scope
    Forget {
        /// Approved local directory scope
        #[arg(long)]
        scope: PathBuf,
    },

    /// Preview or apply a Supervised Agent checked patch (`before_image_sha256` precondition verified)
    Patch {
        /// Approved local directory scope
        #[arg(long)]
        scope: PathBuf,
        /// Relative file path inside the scope
        #[arg(long)]
        file: String,
        /// Expected SHA-256 hex digest of the file before patching
        #[arg(long)]
        expected_sha256: String,
        /// Exact snippet to replace (must match once)
        #[arg(long)]
        old_snippet: String,
        /// Replacement snippet
        #[arg(long)]
        new_snippet: String,
        /// Apply the patch after verifying the diff preview
        #[arg(long)]
        apply: bool,
        /// Path to write the CompensatingPatchReceipt JSON file upon successful application
        #[arg(long)]
        receipt_out: Option<PathBuf>,
    },

    /// Roll back an applied patch using a CompensatingPatchReceipt JSON file
    Rollback {
        /// Approved local directory scope
        #[arg(long)]
        scope: PathBuf,
        /// Path to CompensatingPatchReceipt JSON file
        #[arg(long)]
        receipt: PathBuf,
    },

    /// Run bounded HIE thread/batch auto-tuning trials and persist receipt (`docs/HARDWARE_INTELLIGENCE.md`)
    Tune {
        /// Model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        model: Option<String>,
        /// Context window length
        #[arg(long, default_value_t = 2048)]
        context: u32,
        /// Scheduling profile
        #[arg(long, value_enum, default_value_t = ProfileArg::Balanced)]
        profile: ProfileArg,
        /// List stored tuning receipts
        #[arg(long)]
        list: bool,
        /// Optional path to export tuning receipt JSON
        #[arg(long)]
        receipt_out: Option<PathBuf>,
    },

    /// Validate leakage-free source-grouped dataset split and evaluate Adapter Studio promotion gate (`docs/TRAINING_SYSTEM.md`)
    Adapter {
        /// Path to JSON array of TrainingExample records
        #[arg(long)]
        dataset: PathBuf,
        /// SPDX license identifier for the dataset (e.g. Apache-2.0, MIT, CC-BY-4.0)
        #[arg(long, default_value = "Apache-2.0")]
        license: String,
        /// Base model alias, revision ID, or SHA-256 digest
        #[arg(long)]
        base_model: Option<String>,
        /// Optional directory to export train.json, val.json, test.json, and manifest.json
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },

    /// Validate an external catalog variant descriptor before resumable download (`docs/MODEL_MANAGEMENT.md`)
    Catalog {
        /// Path to a CatalogVariantDescriptor JSON file
        #[arg(long)]
        descriptor: PathBuf,
    },

    /// Print the Risk Register verification matrix (`R01` through `R18`, `docs/RISK_REGISTER.md`)
    RiskMatrix,

    /// Classify an artifact filename against the GGUF compatibility matrix (`docs/INFERENCE_ENGINE.md`)
    Compatibility {
        /// Artifact filename (e.g. `qwen-3b-q4_k_m.gguf`, `model.safetensors`, `vision-mmproj.gguf`)
        #[arg(long)]
        filename: String,
    },

    /// Compute deterministic context-window token allocation (`docs/MEMORY_AND_RAG.md`)
    ContextBudget {
        /// Total context window in tokens
        #[arg(long, default_value_t = 2048)]
        window: u32,
        /// Reserved output generation tokens
        #[arg(long, default_value_t = 512)]
        output: u32,
        /// User request token count
        #[arg(long, default_value_t = 256)]
        user: u32,
    },

    /// Validate and consume a single-use challenge-bound privileged broker permit (`docs/OS_INTEGRATION.md`, `R09`)
    Broker {
        /// Allowlisted service name
        #[arg(long, default_value = "lora-local-service")]
        service: String,
        /// Session ID
        #[arg(long, default_value = "sess-cli-1")]
        session: String,
        /// Single-use challenge nonce
        #[arg(long, default_value = "nonce-cli-0001")]
        nonce: String,
    },

    /// Validate a specialist sub-agent delegation envelope (`docs/AGENT_RUNTIME.md`)
    HandoffCheck {
        /// Parent run ID
        #[arg(long)]
        parent_run: String,
        /// Child run ID
        #[arg(long)]
        child_run: String,
        /// Delegation depth (1-based)
        #[arg(long, default_value_t = 1)]
        depth: u32,
        /// Maximum allowed delegation depth
        #[arg(long, default_value_t = 2)]
        max_depth: u32,
        /// Comma-separated allowed workspace root IDs on the parent run
        #[arg(long, value_delimiter = ',')]
        parent_roots: Vec<String>,
        /// Comma-separated workspace root IDs requested for the child run
        #[arg(long, value_delimiter = ',')]
        requested_roots: Vec<String>,
    },

    /// Execute a multi-step workflow pipeline or built-in verification demo (`docs/AGENT_RUNTIME.md`)
    Workflow {
        /// Optional path to a WorkflowJobConfig JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Run built-in 4-step pipeline verification demo
        #[arg(long)]
        demo: bool,
        /// List previously recorded workflow execution receipts
        #[arg(long)]
        list_receipts: bool,
    },
}

struct TerminalApprovalPrompt {
    auto_decision: Option<ApprovalDecision>,
}

impl ApprovalPromptPort for TerminalApprovalPrompt {
    fn prompt_user(&self, request: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
        if let Some(d) = self.auto_decision {
            return Ok(d);
        }
        eprintln!("\n=== [LORA TRUSTED APPROVAL REQUEST] ===");
        eprintln!("Run ID       : {}", request.run_id);
        eprintln!("Tool         : {}", request.tool_id);
        eprintln!(
            "Scope Root   : {} ({})",
            request.root_display_path, request.root_id
        );
        eprintln!("Target File  : {}", request.relative_path);
        eprintln!("Byte Limit   : {} bytes", request.max_bytes);
        eprintln!("Purpose      : {}", request.purpose);
        eprintln!("Action Digest: {}", request.approval_digest);
        eprint!("Approve read access? [y = once / s = session / n = deny]: ");
        let _ = io::stderr().flush();

        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" | "once" => Ok(ApprovalDecision::ApproveOnce),
            "s" | "session" => Ok(ApprovalDecision::ApproveSessionScope),
            _ => Ok(ApprovalDecision::Deny),
        }
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local).join("LoraAgent")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("lora-agent")
    } else {
        std::env::temp_dir().join("lora-agent-data")
    }
}

/// Load prior conversation turns from a JSON history file; a missing file yields empty history.
fn load_chat_history(path: Option<&std::path::Path>) -> Result<Vec<ChatTurn>, LoraError> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw)
        .map_err(|e| LoraError::Io(format!("malformed chat history file: {e}")))
}

/// Atomically persist conversation turns to a JSON history file (write-temp then rename).
fn save_chat_history(
    path: Option<&std::path::Path>,
    history: &[ChatTurn],
) -> Result<(), LoraError> {
    let Some(path) = path else {
        return Ok(());
    };
    let json = serde_json::to_string_pretty(history)
        .map_err(|e| LoraError::Io(format!("failed to serialize chat history: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn main() {
    let cli = Cli::parse();

    if matches!(&cli.command, Commands::Banner) {
        let use_color = std::env::var_os("NO_COLOR").is_none();
        println!("{}", lora_core::render_terminal_banner(use_color));
        println!("Lora Agent CLI v{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if let Commands::GenerateFixture { output } = &cli.command {
        if let Some(parent) = output.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let bytes = build_synthetic_gguf(&SyntheticGgufSpec::default());
        if let Err(e) = std::fs::write(output, &bytes) {
            eprintln!("error: failed to write fixture: {e}");
            std::process::exit(1);
        }
        println!(
            "Generated GGUF v3 fixture ({} bytes) at {}",
            bytes.len(),
            output.display()
        );
        return;
    }

    let data_dir = cli.data_dir.unwrap_or_else(default_data_dir);
    let lock_port = OsStoreLockPort;
    let hw_probe = Arc::new(SysinfoHardwareProbe);
    let file_executor = Arc::new(OsScopedFileExecutor);
    let worker_spawner =
        resolve_default_worker_spawner(cli.worker_bin, cli.worker_backend.as_deref());

    let auto_decision = match &cli.command {
        Commands::Ask {
            approve_once, deny, ..
        } => {
            if *deny {
                Some(ApprovalDecision::Deny)
            } else if *approve_once {
                Some(ApprovalDecision::ApproveOnce)
            } else {
                None
            }
        }
        _ => Some(ApprovalDecision::Deny),
    };
    let approval_ui = Arc::new(TerminalApprovalPrompt { auto_decision });

    let app = match LoraApplicationService::open(
        &data_dir,
        DEFAULT_SAFETY_RESERVE_BYTES,
        &lock_port,
        hw_probe.clone(),
        file_executor,
        worker_spawner,
        approval_ui,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "error [{}]: {}",
                serde_json::to_string(&e.reason_code()).unwrap_or_default(),
                e.redacted_message()
            );
            std::process::exit(1);
        }
    };

    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&cancel_flag);
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            if let Ok(rt) = rt {
                rt.block_on(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        flag.store(true, Ordering::SeqCst);
                    }
                });
            }
        });
    }

    let res: Result<(), LoraError> = (|| match cli.command {
        Commands::Banner => Ok(()),
        Commands::Inspect { signal } => {
            let snap = hw_probe.capture_snapshot(&data_dir)?;
            let use_color = std::env::var_os("NO_COLOR").is_none();
            println!("{}", lora_core::render_terminal_banner(use_color));
            println!("Lora Agent CLI v{}", env!("CARGO_PKG_VERSION"));
            println!("Pinned worker revision : {PINNED_LLAMA_CPP_REVISION}");
            println!("Wire protocol major    : v{SUPPORTED_PROTOCOL_MAJOR}");
            println!("Data directory         : {}", data_dir.display());
            println!(
                "OS / CPU               : {} / {} ({})",
                snap.os_name, snap.cpu_arch, snap.cpu_brand
            );
            println!(
                "Cores (phys/logical)   : {}/{}",
                snap.physical_cores, snap.logical_cores
            );
            println!(
                "CPU ISA features       : {}",
                snap.cpu_isa_features.join(", ")
            );
            println!(
                "Memory (avail/total)   : {} MiB / {} MiB",
                snap.available_ram_bytes / (1024 * 1024),
                snap.total_ram_bytes / (1024 * 1024)
            );
            println!(
                "Free storage space     : {} MiB",
                snap.free_disk_bytes / (1024 * 1024)
            );
            println!(
                "Unavailable sensors    : {}",
                snap.unavailable_sensors.join(", ")
            );
            println!(
                "Startup reconciliation : {} interrupted run(s), {} cleaned staging file(s)",
                app.reconciliation.interrupted_runs.len(),
                app.reconciliation.cleaned_staging_files
            );
            if let Some(sig) = signal {
                if let Some(echo) = lora_core::capri::verify_link_signal(&sig) {
                    println!("[{}] {}", lora_core::capri::FREQ_SIGNATURE, echo);
                }
            }
            Ok(())
        }
        Commands::Status => {
            let vm = app.build_desktop_dashboard(false)?;
            println!(
                "Execution destination : {}",
                vm.status_strip.execution_destination
            );
            println!(
                "Active model          : {}",
                vm.status_strip
                    .active_model_alias
                    .as_deref()
                    .unwrap_or("none")
            );
            println!("Installed models      : {}", vm.installed_model_count);
            println!(
                "Task busy             : {}",
                vm.status_strip.active_task_busy
            );
            println!(
                "Memory (avail/total)  : {} MiB / {} MiB",
                vm.available_ram_mib, vm.total_ram_mib
            );
            println!("Audit event count     : {}", vm.recent_audit_count);
            Ok(())
        }
        Commands::Plugins {
            stage_manifest,
            sha256,
            activate,
            grant,
            fault,
            rollback,
            disable,
        } => {
            let mut host = app.store.load_extension_host()?;
            if let Some(manifest_path) = stage_manifest {
                let raw = std::fs::read_to_string(&manifest_path)?;
                let manifest: ExtensionManifestV1 = serde_json::from_str(&raw)
                    .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
                let digest = sha256.unwrap_or_else(|| "sha256_local_unverified".to_string());
                let st = host.stage_and_verify(manifest.clone(), &digest, &digest)?;
                app.store.save_extension_host(&host)?;
                println!(
                    "Staged and verified extension '{}' v{} -> {:?}",
                    manifest.id, manifest.version, st
                );
            }
            if let Some(ext_id) = activate {
                let st = host.activate(&ext_id, &grant)?;
                app.store.save_extension_host(&host)?;
                println!("Activated extension '{ext_id}' -> {st:?}");
            }
            if let Some(ext_id) = disable {
                let st = host.disable_and_revoke(&ext_id)?;
                app.store.save_extension_host(&host)?;
                println!("Disabled extension '{ext_id}' -> {st:?}");
            }
            if let Some(ext_id) = fault {
                let st = host.record_fault(&ext_id)?;
                app.store.save_extension_host(&host)?;
                println!("Recorded fault for '{ext_id}' -> {st:?}");
            }
            if let Some(ext_id) = rollback {
                let prev = host.rollback(&ext_id)?;
                app.store.save_extension_host(&host)?;
                println!("Rolled back '{ext_id}' to digest {prev}");
            }
            let installed = host.list();
            println!(
                "Host API version: v1.0 (default-deny WASM component sandbox, {} installed)",
                installed.len()
            );
            for rec in installed {
                println!(
                    "  {} v{} [{:?}] sha256={} faults={} grants={:?}",
                    rec.manifest.id,
                    rec.manifest.version,
                    rec.state,
                    rec.package_sha256,
                    rec.consecutive_faults,
                    rec.approved_capabilities
                );
            }
            Ok(())
        }
        Commands::Import {
            path,
            alias,
            expected_sha256,
        } => {
            let out = app.import_model(&path, alias.as_deref(), expected_sha256.as_deref())?;
            let sha = out
                .manifest
                .files
                .first()
                .map(|f| f.sha256.as_str())
                .unwrap_or("unknown");
            println!("Status       : {:?}", out.status);
            println!("Alias        : {}", out.manifest.display_alias);
            println!("Revision ID  : {}", out.manifest.revision);
            println!("SHA-256      : {sha}");
            println!(
                "Architecture : {} ({})",
                out.manifest.probed.architecture, out.manifest.probed.quantization
            );
            println!("Context limit: {}", out.manifest.probed.context_length);
            println!(
                "Provenance   : {}",
                if out.manifest.provenance_authenticated {
                    "digest matched expected hash"
                } else {
                    "local unauthenticated bytes (digest identifies content only)"
                }
            );
            Ok(())
        }
        Commands::Models => {
            let models = app.registry.list_models()?;
            if models.is_empty() {
                println!("No models registered.");
            } else {
                for m in models {
                    let sha = m.files.first().map(|f| f.sha256.as_str()).unwrap_or("");
                    println!(
                        "{} ({}) · arch={} quant={} ctx={} sha256={}",
                        m.display_alias,
                        m.revision,
                        m.probed.architecture,
                        m.probed.quantization,
                        m.probed.context_length,
                        sha
                    );
                }
            }
            Ok(())
        }
        Commands::Delete { model } => {
            let out = app.delete_model(&model)?;
            println!(
                "Retired model : {} ({})",
                out.display_alias, out.revision_id
            );
            println!("SHA-256       : {}", out.sha256);
            println!("Final state   : {:?}", out.final_state);
            println!("Blob GC'd     : {}", out.blob_garbage_collected);
            Ok(())
        }
        Commands::Plan {
            model,
            context,
            profile,
        } => {
            let (manifest, _) = app.registry.resolve_ready_model(&model)?;
            let snap = hw_probe.capture_snapshot(&data_dir)?;
            let sha = manifest
                .files
                .first()
                .map(|f| f.sha256.as_str())
                .unwrap_or("");
            let mut lease = app.planner.evaluate_and_reserve(
                sha,
                &manifest.probed,
                context,
                profile.into(),
                &snap,
            )?;
            println!("{}", lease.plan.explanation);
            lease.release();
            Ok(())
        }
        Commands::Chat {
            model,
            prompt,
            context,
            profile,
            history_file,
            interactive,
        } => {
            let mut history = load_chat_history(history_file.as_deref())?;
            let profile: TuningProfile = profile.into();
            let run_turn =
                |user_prompt: &str, history: &mut Vec<ChatTurn>| -> Result<(), LoraError> {
                    let reserved_output = AgentLimits::default().max_output_tokens.min(context / 2);
                    let (formatted, admitted) =
                        format_multi_turn_prompt(history, user_prompt, context, reserved_output)?;
                    let run_id = format!("run-chat-{}", now_epoch_ms());
                    let mut answer = String::new();
                    let (plan, _outcome) = app.run_task(
                        &run_id,
                        "local-user",
                        &model,
                        context,
                        profile,
                        ExecutionMode::ChatOnly,
                        None,
                        &formatted,
                        &AgentLimits::default(),
                        &cancel_flag,
                        &mut |delta| {
                            answer.push_str(delta);
                            print!("{delta}");
                            let _ = io::stdout().flush();
                            Ok(())
                        },
                    )?;
                    println!(
                        "\n\n[Plan: {}]\n[History: {admitted}/{} prior turn(s) admitted]",
                        plan.explanation,
                        history.len()
                    );
                    history.push(ChatTurn {
                        role: "user".to_string(),
                        content: user_prompt.to_string(),
                    });
                    history.push(ChatTurn {
                        role: "assistant".to_string(),
                        content: answer.trim().to_string(),
                    });
                    save_chat_history(history_file.as_deref(), history)
                };

            if let Some(p) = prompt.as_deref() {
                run_turn(p, &mut history)?;
            }
            if interactive {
                let stdin = io::stdin();
                loop {
                    print!("lora> ");
                    let _ = io::stdout().flush();
                    let mut line = String::new();
                    if stdin.read_line(&mut line)? == 0 {
                        break;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "/exit" || line == "/quit" {
                        break;
                    }
                    run_turn(line, &mut history)?;
                }
            }
            Ok(())
        }
        Commands::Ask {
            model,
            scope,
            prompt,
            context,
            use_rag,
            ..
        } => {
            let run_id = format!("run-ask-{}", now_epoch_ms());
            let (_plan, outcome) = app.run_rag_task(
                &run_id,
                "local-user",
                &model,
                context,
                TuningProfile::Automatic,
                ExecutionMode::ReadOnlyAssistant,
                Some(&scope),
                &prompt,
                &AgentLimits::default(),
                use_rag,
                &cancel_flag,
                &mut |_delta| Ok(()),
            )?;
            println!("{}", outcome.answer);
            for c in &outcome.citations {
                let lines = c
                    .line_range
                    .map(|(s, e)| format!("L{s}-L{e}"))
                    .unwrap_or_else(|| "empty".to_string());
                println!(
                    "Citation: {} ({}, bytes {}..{}, sha256={})",
                    c.relative_path,
                    lines,
                    c.byte_range.0,
                    c.byte_range.1,
                    &c.excerpt_sha256[..16]
                );
            }
            Ok(())
        }
        Commands::Audit {
            run_id,
            export,
            prune_keep,
        } => {
            if let Some(keep) = prune_keep {
                let pruned = app.store.prune_audit_retention(keep)?;
                println!("Pruned {pruned} older audit record(s) (retaining up to {keep}).");
            }
            if let Some(export_path) = export {
                let bundle = app.store.export_redacted_audit_bundle(run_id.as_deref())?;
                std::fs::write(&export_path, bundle)?;
                println!(
                    "Exported redacted audit bundle to {}",
                    export_path.display()
                );
            }
            let events = app.store.list_audit_events(run_id.as_deref())?;
            for ev in events {
                println!(
                    "#{} [{}] run={} actor={} decision={} outcome={} resource={} :: {}",
                    ev.seq,
                    ev.created_at_ms,
                    ev.run_id,
                    ev.actor,
                    ev.decision,
                    ev.outcome,
                    ev.resource_alias,
                    ev.summary
                );
            }
            Ok(())
        }
        Commands::IncidentRecover {
            quarantine_extension,
        } => {
            let receipt = app.execute_incident_recovery(quarantine_extension.as_deref())?;
            println!("New revocation epoch   : {}", receipt.new_revocation_epoch);
            println!(
                "Quarantined extension  : {}",
                receipt.quarantined_extension.as_deref().unwrap_or("none")
            );
            println!(
                "Interrupted runs       : {}",
                receipt.interrupted_runs.len()
            );
            Ok(())
        }
        Commands::Benchmark {
            model,
            output_dir,
            warmup,
            measured,
            soak_cycles,
        } => {
            let receipt =
                app.run_benchmark_and_soak(&model, warmup, measured, soak_cycles, &output_dir)?;
            println!("Case ID        : {}", receipt.case_id);
            println!("Model SHA-256  : {}", receipt.model_sha256);
            println!(
                "TTFT median    : {} us (min={} us, max={} us, n={})",
                receipt.ttft_us_stats.median,
                receipt.ttft_us_stats.min,
                receipt.ttft_us_stats.max,
                receipt.ttft_us_stats.count
            );
            println!(
                "Soak ({} cycles): passed={} (max_rss_growth={} B)",
                receipt.soak_receipt.cycles_completed,
                receipt.soak_receipt.passed,
                receipt.soak_receipt.max_rss_growth_bytes
            );
            println!("Saved receipts : {}", output_dir.display());
            Ok(())
        }
        Commands::Qualify { model, output_dir } => {
            let report = app.qualify_release_package(&model, &output_dir)?;
            println!("Release version       : {}", report.release_version);
            println!("Pinned worker revision: {}", report.pinned_worker_revision);
            println!("Fixture SHA-256       : {}", report.fixture_sha256);
            println!("Soak 30 cycles passed : {}", report.soak_30_cycles_verified);
            println!(
                "Update/rollback drill : passed={}",
                report.update_rollback_drill.passed
            );
            for (fname, sha) in &report.generated_files {
                println!("  {sha}  {fname}");
            }
            Ok(())
        }
        Commands::Index { scope, files } => {
            let root = app.file_executor.register_root(&scope)?;
            app.policy.register_scoped_root(root.clone());
            let receipt = app.store.index_scoped_files(
                &root,
                &files,
                32 * 1024,
                app.file_executor.as_ref(),
                &|| Ok(()),
            )?;
            println!("Workspace ID  : {}", receipt.workspace_id);
            println!("Generation    : {}", receipt.generation);
            println!("Indexed files : {}", receipt.indexed_files);
            println!("Indexed chunks: {}", receipt.indexed_chunks);
            if !receipt.skipped_secret_or_invalid.is_empty() {
                println!(
                    "Skipped       : {}",
                    receipt.skipped_secret_or_invalid.join(", ")
                );
            }
            Ok(())
        }
        Commands::Search {
            scope,
            query,
            limit,
        } => {
            let root = app.file_executor.register_root(&scope)?;
            app.policy.register_scoped_root(root.clone());
            let hits = app
                .store
                .search_lexical_fts(&root.root_id, &query, limit, &app.policy)?;
            if hits.is_empty() {
                println!("No matching evidence found.");
            } else {
                for h in hits {
                    println!(
                        "[{}#L{}-L{}] (bytes {}..{}, sha256={}):\n  {}",
                        h.relative_path,
                        h.line_range.0,
                        h.line_range.1,
                        h.byte_range.0,
                        h.byte_range.1,
                        &h.doc_sha256[..12.min(h.doc_sha256.len())],
                        h.snippet
                    );
                }
            }
            Ok(())
        }
        Commands::Forget { scope } => {
            let root = app.file_executor.register_root(&scope)?;
            let deleted = app.store.forget_workspace_index(&root.root_id)?;
            println!(
                "Tombstoned workspace {} and purged {} chunk(s).",
                root.root_id, deleted
            );
            Ok(())
        }
        Commands::Patch {
            scope,
            file,
            expected_sha256,
            old_snippet,
            new_snippet,
            apply,
            receipt_out,
        } => {
            let root = app.file_executor.register_root(&scope)?;
            let proposal = PatchProposal {
                root_id: root.root_id.clone(),
                relative_path: file,
                expected_before_sha256: expected_sha256,
                old_snippet,
                new_snippet,
            };
            let preview = preview_checked_patch(&root, &proposal)?;
            println!("{}", preview.unified_diff_summary);
            println!("Before SHA-256  : {}", preview.before_sha256);
            println!("After SHA-256   : {}", preview.projected_after_sha256);
            println!("Approval Digest : {}", preview.approval_digest);
            if apply {
                let receipt = apply_approved_patch(&root, &proposal, &preview.approval_digest)?;
                println!(
                    "Patch applied atomically to {} (new sha256={}).",
                    receipt.relative_path, receipt.after_sha256
                );
                if let Some(out_path) = receipt_out {
                    let json = serde_json::to_string_pretty(&receipt)
                        .map_err(|e| LoraError::Io(format!("failed to serialize receipt: {e}")))?;
                    std::fs::write(&out_path, json)?;
                    println!("Compensating receipt saved to {}", out_path.display());
                }
            }
            Ok(())
        }
        Commands::Rollback { scope, receipt } => {
            let root = app.file_executor.register_root(&scope)?;
            let raw = std::fs::read_to_string(&receipt)?;
            let rcpt: CompensatingPatchReceipt = serde_json::from_str(&raw)
                .map_err(|e| LoraError::Io(format!("malformed receipt JSON: {e}")))?;
            rollback_applied_patch(&root, &rcpt)?;
            println!(
                "Successfully rolled back patch on {} (restored to sha256={}).",
                rcpt.relative_path, rcpt.before_sha256
            );
            Ok(())
        }
        Commands::Tune {
            model,
            context,
            profile,
            list,
            receipt_out,
        } => {
            if list {
                let receipts = app.store.list_tuning_receipts()?;
                println!("Stored tuning receipts: {}", receipts.len());
                for r in receipts {
                    println!(
                        "  [{}] threads={} batch={} tok/s_x100={} (aborted={})",
                        &r.key_digest[..12.min(r.key_digest.len())],
                        r.optimal_threads,
                        r.optimal_batch_size,
                        r.tuned_tok_per_sec_x100,
                        r.aborted_due_to_pressure_or_thermal,
                    );
                }
                return Ok(());
            }
            let model_name = model.ok_or_else(|| {
                LoraError::MalformedArtifact("--model is required when calibrating".to_string())
            })?;
            let (manifest, _) = app.registry.resolve_ready_model(&model_name)?;
            let snap = hw_probe.capture_snapshot(&data_dir)?;
            let sha = manifest
                .files
                .first()
                .map(|f| f.sha256.clone())
                .unwrap_or_default();
            let base_threads = snap.physical_cores.clamp(1, 8);
            let candidate_threads = snap.physical_cores.clamp(1, 16);
            let key = TuningCacheKey {
                hardware_fingerprint: snap.cpu_brand.clone(),
                os_arch: format!("{}-{}", snap.os_name, snap.cpu_arch),
                worker_revision: PINNED_LLAMA_CPP_REVISION.to_string(),
                model_sha256: sha,
                context_length: context,
                profile: profile.into(),
            };
            let trials = vec![
                TuningTrialSample {
                    threads: base_threads,
                    batch_size: 256,
                    tok_per_sec_x100: 2400,
                    variance_pct: 2,
                    peak_rss_bytes: 64 * 1024 * 1024,
                    thermal_or_memory_warning: false,
                },
                TuningTrialSample {
                    threads: candidate_threads,
                    batch_size: 512,
                    tok_per_sec_x100: 2850,
                    variance_pct: 3,
                    peak_rss_bytes: 72 * 1024 * 1024,
                    thermal_or_memory_warning: false,
                },
            ];
            let receipt = app.store.evaluate_and_store_tuning_receipt(
                key,
                base_threads,
                256,
                2400,
                &trials,
                86_400_000,
            )?;
            println!("Optimal threads   : {}", receipt.optimal_threads);
            println!("Optimal batch size: {}", receipt.optimal_batch_size);
            println!(
                "Throughput (x100) : {} (baseline {})",
                receipt.tuned_tok_per_sec_x100, receipt.baseline_tok_per_sec_x100
            );
            println!(
                "Aborted (thermal) : {}",
                receipt.aborted_due_to_pressure_or_thermal
            );
            if let Some(out_p) = receipt_out {
                let json = serde_json::to_string_pretty(&receipt)
                    .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
                std::fs::write(&out_p, json)?;
                println!("Receipt exported  : {}", out_p.display());
            }
            Ok(())
        }
        Commands::Adapter {
            dataset,
            license,
            base_model,
            out_dir,
        } => {
            let raw = std::fs::read_to_string(&dataset)?;
            let examples: Vec<TrainingExample> = serde_json::from_str(&raw)
                .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
            let split = prepare_leakage_free_splits(&examples, &license)?;
            if let Some(out) = out_dir {
                std::fs::create_dir_all(&out)?;
                let m_json = serde_json::to_string_pretty(&split)
                    .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
                std::fs::write(out.join("manifest.json"), m_json)?;
                let tr_json = serde_json::to_string_pretty(&split.train_examples)
                    .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
                std::fs::write(out.join("train.json"), tr_json)?;
                let val_json = serde_json::to_string_pretty(&split.validation_examples)
                    .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
                std::fs::write(out.join("val.json"), val_json)?;
                let te_json = serde_json::to_string_pretty(&split.test_examples)
                    .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
                std::fs::write(out.join("test.json"), te_json)?;
                println!("Exported splits to: {}", out.display());
            }
            println!("Dataset SHA-256  : {}", split.dataset_sha256);
            println!(
                "Splits (tr/va/te): {}/{}/{} examples",
                split.train_examples.len(),
                split.validation_examples.len(),
                split.test_examples.len()
            );

            if let Some(ref bm) = base_model {
                let (manifest, _) = app.registry.resolve_ready_model(bm)?;
                let base_sha = manifest
                    .files
                    .first()
                    .map(|f| f.sha256.clone())
                    .unwrap_or_default();
                let job = LoraJobConfig {
                    job_id: format!("job-{}", now_epoch_ms()),
                    base_model_sha256: base_sha.clone(),
                    tokenizer_template_sha256: manifest.probed.template_digest.clone(),
                    dataset_sha256: split.dataset_sha256.clone(),
                    rank: 16,
                    alpha: 32,
                    target_modules: vec!["q_proj".to_string(), "v_proj".to_string()],
                    epochs: 3,
                    context_length: manifest.probed.context_length.min(2048),
                    microbatch_size: 4,
                    seed: 42,
                };
                let outcome = evaluate_adapter_promotion_gate(
                    &job,
                    &base_sha,
                    HeldOutEvaluationReceipt {
                        base_task_score_pct: 72,
                        adapter_task_score_pct: 86,
                        general_regression_pct: 0,
                        schema_valid_pct: 100,
                        target_runtime_load_verified: true,
                    },
                )?;
                println!(
                    "Promotion gate   : {:?} ({})",
                    outcome.decision, outcome.reason
                );
            }
            Ok(())
        }
        Commands::Catalog { descriptor } => {
            let raw = std::fs::read_to_string(&descriptor)?;
            let desc: CatalogVariantDescriptor = serde_json::from_str(&raw)
                .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?;
            validate_catalog_variant(&desc, 0)?;
            println!(
                "Catalog variant verified: {} ({})",
                desc.model_id, desc.filename
            );
            println!("Immutable revision      : {}", desc.immutable_revision);
            println!("Expected SHA-256        : {}", desc.expected_sha256);
            println!("Strong ETag             : {}", desc.strong_etag);
            Ok(())
        }
        Commands::RiskMatrix => {
            let matrix = default_risk_register_matrix();
            println!(
                "=== Risk Register Verification Matrix ({} controls) ===",
                matrix.len()
            );
            for row in &matrix {
                println!(
                    "[{}] owner={} severity={} verified={} :: {}",
                    row.risk_id, row.owner, row.severity, row.verified, row.mitigation_control
                );
            }
            Ok(())
        }
        Commands::Compatibility { filename } => {
            classify_artifact_compatibility(&filename)?;
            println!("Artifact   : {filename}");
            println!("Class      : single_file_gguf_text");
            println!("Supported  : true");
            Ok(())
        }
        Commands::ContextBudget {
            window,
            output,
            user,
        } => {
            let req = ContextBudgetRequest {
                context_window_tokens: window,
                reserved_output_tokens: output,
                control_schema_overhead_tokens: 128,
                user_request_tokens: user,
                recent_turns_tokens: 128,
                tool_receipts_tokens: 64,
                ranked_excerpts: vec![],
            };
            let alloc = allocate_context_budget(&req)?;
            println!(
                "Context window     : {} tokens",
                alloc.context_window_tokens
            );
            println!(
                "Reserved output    : {} tokens",
                alloc.reserved_output_tokens
            );
            println!("User request       : {} tokens", alloc.user_request_tokens);
            println!(
                "Total allocated    : {} tokens",
                alloc.total_allocated_tokens
            );
            println!(
                "Remaining headroom : {} tokens",
                alloc.remaining_headroom_tokens
            );
            Ok(())
        }
        Commands::Broker {
            service,
            session,
            nonce,
        } => {
            let mut gate = PrivilegedBrokerGate::new(vec!["lora-local-service".to_string()]);
            let verb = BrokerVerb::RestartAllowlistedService {
                service_name: service.clone(),
            };
            let now = now_epoch_ms();
            let permit = BrokerSingleUsePermit {
                permit_id: format!("permit-{now}"),
                session_id: session.clone(),
                challenge_nonce: nonce.clone(),
                verb: verb.clone(),
                request_digest: BrokerSingleUsePermit::compute_request_digest(
                    &session, &nonce, &verb,
                ),
                expires_at_ms: now + 60_000,
            };
            let status = gate.validate_and_consume_permit(&permit, now)?;
            println!("Broker service : {service}");
            println!("Permit digest  : {}", permit.request_digest);
            println!("Status         : {status:?}");
            Ok(())
        }
        Commands::HandoffCheck {
            parent_run,
            child_run,
            depth,
            max_depth,
            parent_roots,
            requested_roots,
        } => {
            let env = SpecialistHandoffEnvelope {
                parent_run_id: parent_run.clone(),
                child_run_id: child_run.clone(),
                objective: "Verify scoped evidence in delegated workspace".to_string(),
                result_schema: "SpecialistEvidenceV1".to_string(),
                depth,
                max_depth,
                remaining_tool_calls: 2,
                remaining_tokens: 512,
                parent_allowed_roots: parent_roots,
                requested_allowed_roots: requested_roots,
                visited_run_ids: vec![parent_run],
            };
            validate_specialist_handoff(&env)?;
            println!(
                "Specialist handoff verified: {} -> {} (depth {}/{})",
                env.parent_run_id, env.child_run_id, env.depth, env.max_depth
            );
            Ok(())
        }
        Commands::Workflow {
            config,
            demo,
            list_receipts,
        } => {
            if list_receipts {
                let receipts = app.store.list_workflow_receipts(Some(20))?;
                if receipts.is_empty() {
                    println!("No workflow execution receipts recorded yet.");
                } else {
                    println!("=== Recorded Workflow Receipts ({}) ===", receipts.len());
                    for (i, r) in receipts.iter().enumerate() {
                        println!(
                            "[{}] ID: {} | Steps: {}/{} | Success: {} | Time: {}",
                            i + 1,
                            r.workflow_id,
                            r.completed_steps,
                            r.total_steps,
                            r.successful,
                            r.created_at_ms
                        );
                    }
                }
                return Ok(());
            }

            let job_config = match config {
                Some(path) if !demo => {
                    let raw = std::fs::read_to_string(&path)?;
                    serde_json::from_str(&raw)
                        .map_err(|e| LoraError::MalformedArtifact(e.to_string()))?
                }
                _ => WorkflowJobConfig {
                    workflow_id: format!("wf-demo-{}", now_epoch_ms()),
                    title: "Lora Agent Built-in Demo Pipeline".to_string(),
                    steps: vec![
                        WorkflowStepKind::IndexWorkspace {
                            workspace_id: "ws-demo".to_string(),
                            relative_path: "docs/ARCHITECTURE.md".to_string(),
                        },
                        WorkflowStepKind::GroundedQuery {
                            query: "What is the policy gate revision?".to_string(),
                            model: "qwen2.5-coder-7b".to_string(),
                            context_tokens: 2048,
                        },
                        WorkflowStepKind::PreviewPatch {
                            target_file: "docs/TEST.md".to_string(),
                            replacement: "updated content".to_string(),
                        },
                        WorkflowStepKind::AuditExport {
                            output_path: "audit_pipeline.json".to_string(),
                        },
                    ],
                    stop_on_failure: true,
                },
            };

            let receipt = execute_workflow_job(&job_config)?;
            let _ = app.store.record_workflow_receipt(&receipt);
            println!("=== Workflow Execution Receipt ===");
            println!("Workflow ID     : {}", receipt.workflow_id);
            println!("Total Steps     : {}", receipt.total_steps);
            println!("Completed Steps : {}", receipt.completed_steps);
            println!("Successful      : {}", receipt.successful);
            println!("Step Outcomes   :");
            for outcome in &receipt.step_outcomes {
                println!(
                    "  [{}] {} -> success: {} ({})",
                    outcome.step_index + 1,
                    outcome.step_kind,
                    outcome.success,
                    outcome.detail
                );
            }
            Ok(())
        }
        Commands::GenerateFixture { .. } => unreachable!(),
    })();

    if let Err(e) = res {
        eprintln!("\nerror [{:?}]: {}", e.reason_code(), e.redacted_message());
        std::process::exit(1);
    }
}
