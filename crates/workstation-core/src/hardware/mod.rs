// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only Linux hardware snapshots. Unknown readings remain optional.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub mod amd_sysfs;
pub mod cpu;
pub mod nvml;
pub mod vulkan;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComputeBackend {
    Cuda {
        compute_capability: (i32, i32),
        driver_version: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPool {
    pub total_bytes: u64,
    pub available_bytes: Option<u64>,
}

impl MemoryPool {
    pub(super) fn from_usage(total: u64, used: u64) -> Result<Self, String> {
        let available = total
            .checked_sub(used)
            .ok_or("Reported GPU usage exceeds total memory.")?;
        Ok(Self {
            total_bytes: total,
            available_bytes: Some(available),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuFeatures {
    pub avx2: bool,
    pub avx512: bool,
    pub vnni: bool,
    pub amx: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuProfile {
    pub model: Option<String>,
    pub physical_cores: Option<usize>,
    pub logical_cores: usize,
    pub features: Option<CpuFeatures>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDeviceProfile {
    pub id: Option<String>,
    pub name: String,
    pub vendor: GpuVendor,
    pub dedicated_vram: Option<MemoryPool>,
    pub shared_gtt: Option<MemoryPool>,
    pub supported_backends: Vec<ComputeBackend>,
    pub unified_memory: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeIssue {
    pub component: String,
    pub message: String,
}

impl ProbeIssue {
    pub(super) fn new(component: impl Into<String>, message: impl ToString) -> Self {
        Self {
            component: component.into(),
            message: message.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub profile_hash: Option<String>,
    pub cpu: Option<CpuProfile>,
    pub system_ram: Option<MemoryPool>,
    pub gpus: Vec<GpuDeviceProfile>,
    pub issues: Vec<ProbeIssue>,
    pub captured_at_unix_ms: u64,
}

impl HardwareProfile {
    /// Synchronous probing: async/UI callers must use `spawn_blocking`.
    pub fn detect() -> Self {
        let mut profile = Self::detect_linux(Path::new("/proc"), Path::new("/sys/class/drm"));
        match nvml::probe() {
            Ok((devices, issues)) => {
                profile.issues.extend(issues);
                for device in devices {
                    if let Some(existing) = profile.gpus.iter_mut().find(|gpu| gpu.id == device.id)
                    {
                        *existing = device;
                    } else {
                        profile.gpus.push(device);
                    }
                }
            }
            Err(error) => profile.issues.push(ProbeIssue::new("NVIDIA NVML", error)),
        }
        profile.gpus.sort_by(|a, b| a.id.cmp(&b.id));
        profile.update_hash();
        profile
    }

    fn detect_linux(proc_root: &Path, drm_root: &Path) -> Self {
        let mut issues = Vec::new();
        let cpu = fs::read_to_string(proc_root.join("cpuinfo"))
            .map_err(|e| e.to_string())
            .and_then(|input| cpu::parse_cpu(&input))
            .map_err(|error| issues.push(ProbeIssue::new("CPU", error)))
            .ok();
        if let Some(cpu) = &cpu {
            if cpu.physical_cores.is_none() {
                issues.push(ProbeIssue::new(
                    "CPU",
                    "Physical core topology is unavailable.",
                ));
            }
            if cpu.features.is_none() {
                issues.push(ProbeIssue::new(
                    "CPU",
                    "CPU instruction flags are unavailable.",
                ));
            }
        }
        let system_ram = fs::read_to_string(proc_root.join("meminfo"))
            .map_err(|e| e.to_string())
            .and_then(|input| cpu::parse_memory(&input))
            .map_err(|error| issues.push(ProbeIssue::new("System RAM", error)))
            .ok();
        if system_ram.is_some_and(|pool| pool.available_bytes.is_none()) {
            issues.push(ProbeIssue::new(
                "System RAM",
                "MemAvailable is unavailable; total RAM is still reported.",
            ));
        }
        let (gpus, gpu_issues) = amd_sysfs::probe(drm_root);
        issues.extend(gpu_issues);
        Self {
            profile_hash: None,
            cpu,
            system_ram,
            gpus,
            issues,
            captured_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0),
        }
    }

    pub fn has_readings(&self) -> bool {
        self.cpu.is_some() || self.system_ram.is_some() || !self.gpus.is_empty()
    }

    fn update_hash(&mut self) {
        self.profile_hash = None;
        if self
            .issues
            .iter()
            .any(|issue| issue.component == "GPU inventory")
        {
            return;
        }
        let Some(model) = self.cpu.as_ref().and_then(|cpu| cpu.model.as_ref()) else {
            return;
        };
        let Some(ram) = self.system_ram else {
            return;
        };
        let Some(mut gpu_ids) = self
            .gpus
            .iter()
            .map(|gpu| gpu.id.as_deref())
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        gpu_ids.sort_unstable();
        let mut hash = Sha256::new();
        for field in std::iter::once(std::env::consts::ARCH)
            .chain(std::iter::once(model.as_str()))
            .chain(gpu_ids)
        {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        hash.update(ram.total_bytes.to_le_bytes());
        self.profile_hash = Some(format!("{:x}", hash.finalize()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_ignores_live_memory_time_and_gpu_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut profile = HardwareProfile::detect_linux(&root.join("proc"), &root.join("drm"));
        profile.update_hash();
        let original = profile.profile_hash.clone();
        assert!(original.is_some());
        profile.system_ram.as_mut().unwrap().available_bytes = Some(0);
        profile.captured_at_unix_ms += 1;
        profile.gpus.reverse();
        profile.update_hash();
        assert_eq!(profile.profile_hash, original);
        profile.system_ram.as_mut().unwrap().total_bytes += 1024;
        profile.update_hash();
        assert_ne!(profile.profile_hash, original);
    }

    #[test]
    fn cpu_only_inventory_remains_useful_without_gpu_devices() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut profile =
            HardwareProfile::detect_linux(&root.join("proc"), &root.join("drm-empty"));
        profile.update_hash();
        assert!(profile.has_readings());
        assert!(profile.cpu.is_some());
        assert!(profile.system_ram.is_some());
        assert!(profile.gpus.is_empty());
        assert!(profile.issues.is_empty());
        assert!(profile.profile_hash.is_some());
    }

    #[test]
    fn missing_sources_are_explicit_and_have_no_identity() {
        let root = Path::new("/this-path-does-not-exist-kvlm-test");
        let mut profile = HardwareProfile::detect_linux(root, root);
        profile.update_hash();
        assert!(!profile.has_readings());
        assert!(profile.profile_hash.is_none());
        assert_eq!(profile.issues.len(), 3);
    }

    #[test]
    fn invalid_gpu_usage_is_not_saturated_into_a_fake_reading() {
        assert!(MemoryPool::from_usage(100, 101).is_err());
        assert_eq!(
            MemoryPool::from_usage(100, 100).unwrap().available_bytes,
            Some(0)
        );
    }
}
