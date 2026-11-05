//! Host hardware discovery and OS budget probes (`docs/HARDWARE_INTELLIGENCE.md`, `T5`).

use lora_core::{now_epoch_ms, ChargingState, HardwareProbePort, HostSnapshot, LoraError};
use std::path::Path;
use sysinfo::{Disks, System};

#[derive(Default, Clone)]
pub struct SysinfoHardwareProbe;

impl HardwareProbePort for SysinfoHardwareProbe {
    fn capture_snapshot(&self, storage_dir: &Path) -> Result<HostSnapshot, LoraError> {
        let mut sys = System::new_all();
        sys.refresh_memory();
        sys.refresh_cpu_all();

        let total_ram_bytes = sys.total_memory();
        let available_ram_bytes = sys.available_memory().max(total_ram_bytes / 4);

        let physical_cores = sys.physical_core_count().unwrap_or(1) as u32;
        let logical_cores = sys.cpus().len().max(1) as u32;
        let cpu_brand = sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_else(|| "unknown-cpu".to_string());

        // Only x86 targets push extra features below.
        #[cfg_attr(
            not(any(target_arch = "x86", target_arch = "x86_64")),
            allow(unused_mut)
        )]
        let mut cpu_isa_features = vec![std::env::consts::ARCH.to_string()];
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if std::is_x86_feature_detected!("avx2") {
                cpu_isa_features.push("avx2".to_string());
            }
            if std::is_x86_feature_detected!("fma") {
                cpu_isa_features.push("fma".to_string());
            }
        }

        // Determine free disk space on the volume containing `storage_dir`
        let canonical = storage_dir
            .canonicalize()
            .unwrap_or_else(|_| storage_dir.to_path_buf());
        let disks = Disks::new_with_refreshed_list();
        let mut best_match_len = 0usize;
        let mut free_disk_bytes = 10 * 1024 * 1024 * 1024u64; // conservative 10 GiB fallback if mount unenumerated

        for disk in &disks {
            let mount = disk.mount_point();
            if canonical.starts_with(mount) {
                let m_len = mount.as_os_str().len();
                if m_len >= best_match_len {
                    best_match_len = m_len;
                    free_disk_bytes = disk.available_space();
                }
            }
        }

        Ok(HostSnapshot {
            os_name: format!(
                "{} {}",
                std::env::consts::OS,
                System::os_version().unwrap_or_default()
            )
            .trim()
            .to_string(),
            cpu_arch: std::env::consts::ARCH.to_string(),
            cpu_brand,
            physical_cores: physical_cores.max(1),
            logical_cores: logical_cores.max(1),
            cpu_isa_features,
            total_ram_bytes,
            available_ram_bytes,
            free_disk_bytes,
            charging_state: ChargingState::Unknown,
            sample_timestamp_ms: now_epoch_ms(),
            unavailable_sensors: vec![
                "discrete_vram_budget".to_string(),
                "package_power_watts".to_string(),
            ],
        })
    }
}
