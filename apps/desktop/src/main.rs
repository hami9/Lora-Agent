//! Pure-Rust native multi-pane desktop client (`lora-desktop`) with headless CI diagnostic flags.

use clap::Parser;
use iced::{window, Size};
use lora_core::{
    slice_virtualized_transcript, verify_desktop_snapshot, DesktopScreen, DesktopSnapshot,
    LoraApplicationService, DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_desktop::app::LoraDesktopApp;
use lora_desktop::backend::{AppWorkerEvent, DenyByDefaultDesktopPrompt, GuiApprovalPromptPort};
use lora_desktop::{build_consent_preview, load_window_icon};
use lora_platform::{
    resolve_default_worker_spawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Debug, Parser)]
#[command(
    name = "lora-desktop",
    version,
    about = "Lora Agent pure-Rust native cyberpunk desktop client"
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

    /// Enable experimental lab channels
    #[arg(long)]
    experimental: bool,

    /// Emit initial desktop dashboard view-model JSON and exit (headless diagnostic)
    #[arg(long)]
    dump_view_model: bool,

    /// Emit a cryptographically attested DesktopSnapshot JSON and exit (headless diagnostic)
    #[arg(long)]
    dump_snapshot: bool,

    /// Verify a DesktopSnapshot JSON file and exit with validation status
    #[arg(long)]
    verify_snapshot: Option<PathBuf>,

    /// Emit a WCAG 2.2 AA TrustedConsentDialogViewModel JSON for a proposed relative path and exit
    #[arg(long)]
    preview_consent: Option<String>,

    /// Emit a VirtualizedTranscriptSlice JSON window for `<total_items>,<scroll_offset>,<viewport_capacity>` (`R12`)
    #[arg(long, value_delimiter = ',')]
    slice_transcript: Vec<usize>,
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

    let headless_mode = cli.preview_consent.is_some()
        || cli.slice_transcript.len() == 3
        || cli.verify_snapshot.is_some()
        || cli.dump_snapshot
        || cli.dump_view_model;

    if headless_mode {
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

        if let Some(rel_path) = cli.preview_consent {
            let dialog = build_consent_preview(&data_dir, rel_path);
            println!(
                "{}",
                serde_json::to_string_pretty(&dialog)
                    .expect("serialize TrustedConsentDialogViewModel")
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

        if let Some(verify_path) = cli.verify_snapshot.as_ref() {
            let content = match std::fs::read_to_string(verify_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error reading snapshot file: {e}");
                    std::process::exit(1);
                }
            };
            let snapshot: DesktopSnapshot = match serde_json::from_str(&content) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error parsing DesktopSnapshot JSON: {e}");
                    std::process::exit(1);
                }
            };
            match verify_desktop_snapshot(&snapshot) {
                Ok(()) => println!(
                    "Desktop Snapshot Attestation: verified (digest: {})",
                    snapshot.snapshot_sha256.as_deref().unwrap_or("none")
                ),
                Err(e) => {
                    eprintln!("error [{:?}]: {}", e.reason_code(), e);
                    std::process::exit(1);
                }
            }
            return;
        }

        if cli.dump_snapshot {
            match app.capture_desktop_snapshot(
                DesktopScreen::Dashboard,
                cli.experimental,
                "desktop-cli-session",
            ) {
                Ok(snapshot) => println!(
                    "{}",
                    serde_json::to_string_pretty(&snapshot).expect("serialize DesktopSnapshot")
                ),
                Err(e) => {
                    eprintln!("error [{:?}]: {}", e.reason_code(), e);
                    std::process::exit(1);
                }
            }
            return;
        }

        let vm = match app.build_desktop_dashboard(cli.experimental) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error [{:?}]: {}", e.reason_code(), e);
                std::process::exit(1);
            }
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&vm).expect("serialize desktop view model")
        );
        return;
    }

    // Interactive Pure-Rust Native GUI Mode (multi-pane HUD client via iced 0.14.0)
    let (event_tx, event_rx) = mpsc::channel::<AppWorkerEvent>();
    let approval_port = Arc::new(GuiApprovalPromptPort::new(event_tx.clone()));
    let service = match LoraApplicationService::open(
        &data_dir,
        DEFAULT_SAFETY_RESERVE_BYTES,
        &lock_port,
        hw_probe,
        file_executor,
        worker_spawner,
        approval_port.clone(),
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("error [{:?}]: {}", e.reason_code(), e);
            std::process::exit(1);
        }
    };

    let event_rx_shared = Arc::new(Mutex::new(event_rx));
    let experimental = cli.experimental;

    if let Err(err) = iced::application(
        move || {
            (
                LoraDesktopApp::new(
                    Arc::clone(&service),
                    Some(Arc::clone(&approval_port)),
                    event_tx.clone(),
                    Arc::clone(&event_rx_shared),
                    experimental,
                ),
                iced::Task::none(),
            )
        },
        LoraDesktopApp::update,
        LoraDesktopApp::view,
    )
    .title(LoraDesktopApp::title)
    .theme(LoraDesktopApp::theme)
    .subscription(LoraDesktopApp::subscription)
    .window(window::Settings {
        size: Size::new(1380.0, 860.0),
        min_size: Some(Size::new(980.0, 620.0)),
        visible: true,
        icon: load_window_icon(),
        ..Default::default()
    })
    .centered()
    .antialiasing(true)
    .run()
    {
        eprintln!("native desktop runtime error: {err}");
        std::process::exit(1);
    }
}
