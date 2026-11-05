//! Desktop UX composition root (`docs/DESKTOP_UX.md`).
//! Exposes the local-first desktop dashboard state, hardware telemetry snapshot, and WCAG 2.2 AA
//! trusted consent view-models to the desktop shell without ambient filesystem or network privileges.

use clap::Parser;
use lora_core::{
    ApprovalDecision, ApprovalPromptPort, ApprovalRequest, LoraApplicationService, LoraError,
    DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_platform::{
    InProcessWorkerSpawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::path::PathBuf;
use std::sync::Arc;

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

    /// Enable experimental screens in the status strip
    #[arg(long)]
    experimental: bool,

    /// Emit initial desktop dashboard view-model JSON and exit
    #[arg(long)]
    dump_view_model: bool,
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
    let worker_spawner = Arc::new(InProcessWorkerSpawner::default());
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
