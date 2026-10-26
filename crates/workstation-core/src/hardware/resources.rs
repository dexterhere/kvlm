// SPDX-License-Identifier: GPL-3.0-or-later
//! System/device-wide memory observations, never attributed to an inference process.

use serde::{Deserialize, Serialize};

pub const MAX_GPUS: usize = 16;
#[derive(Clone, Debug, Default)]
pub struct MemorySample {
    pub system_ram_nonavailable_bytes: Option<u64>,
    pub gpus: Vec<GpuMemorySample>,
}
#[derive(Clone, Debug)]
pub struct GpuMemorySample {
    pub id: String,
    pub name: String,
    pub vram_used_bytes: Option<u64>,
    pub gtt_used_bytes: Option<u64>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSummary {
    pub interval_ms: u64,
    pub window_ms: u64,
    pub samples: u64,
    pub system_ram_samples: u64,
    pub peak_system_ram_nonavailable_bytes: Option<u64>,
    pub gpus: Vec<GpuMemoryPeak>,
    pub probe_failed: bool,
    pub probe_in_flight: bool,
    pub limit_reached: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuMemoryPeak {
    pub id: String,
    pub name: String,
    pub vram_samples: u64,
    pub peak_vram_used_bytes: Option<u64>,
    pub gtt_samples: u64,
    pub peak_gtt_used_bytes: Option<u64>,
}

use super::{GpuVendor, HardwareProfile, MemoryPool, amd_sysfs, cpu, nvml};
use nvml_wrapper::Nvml;
use std::{collections::BTreeSet, path::PathBuf};

struct Target {
    id: String,
    name: String,
    vendor: GpuVendor,
    bus: String,
}
/// Inventory is fixed to the run snapshot; NVML initializes lazily once per run.
/// Call `sample` only from a blocking worker.
pub struct MemoryProbe {
    targets: Vec<Target>,
    nvml: Option<Result<Nvml, ()>>,
    pci_root: PathBuf,
}
impl MemoryProbe {
    pub fn new(profile: &HardwareProfile) -> Self {
        let mut seen = BTreeSet::new();
        let targets = profile
            .gpus
            .iter()
            .filter_map(|gpu| {
                let id = gpu.id.as_deref()?;
                let bus = id.strip_prefix("pci:")?;
                if amd_sysfs::pci_id(bus).as_deref() != Some(id) || !seen.insert(id) {
                    return None;
                }
                Some(Target {
                    id: id.into(),
                    name: gpu.name.chars().take(256).collect(),
                    vendor: gpu.vendor,
                    bus: bus.into(),
                })
            })
            .take(MAX_GPUS)
            .collect();
        Self {
            targets,
            nvml: None,
            pci_root: PathBuf::from("/sys/bus/pci/devices"),
        }
    }
    pub fn sample(&mut self) -> MemorySample {
        let ram = cpu::read_memory().ok().and_then(nonavailable);
        self.sample_devices(ram)
    }
    fn sample_devices(&mut self, ram: Option<u64>) -> MemorySample {
        if self.nvml.is_none()
            && self
                .targets
                .iter()
                .any(|target| target.vendor == GpuVendor::Nvidia)
        {
            self.nvml = Some(Nvml::init().map_err(|_| ()));
        }
        let gpus = self
            .targets
            .iter()
            .map(|target| {
                let (vram, gtt) = match target.vendor {
                    GpuVendor::Amd => {
                        let path = self.pci_root.join(&target.bus);
                        let mut issues = Vec::new();
                        let vram = amd_sysfs::read_pool(&path, "vram", "GPU", &mut issues)
                            .and_then(nonavailable);
                        let gtt = amd_sysfs::read_pool(&path, "gtt", "GPU", &mut issues)
                            .and_then(nonavailable);
                        (vram, gtt)
                    }
                    GpuVendor::Nvidia => {
                        let used = self
                            .nvml
                            .as_ref()
                            .and_then(|library| library.as_ref().ok())
                            .and_then(|library| {
                                library.device_by_pci_bus_id(target.bus.as_str()).ok()
                            })
                            .and_then(|device| device.memory_info().ok())
                            .filter(|memory| {
                                nvml::memory_reading(memory.total, memory.used, memory.free).is_ok()
                            })
                            .map(|memory| memory.used);
                        (used, None)
                    }
                    _ => (None, None),
                };
                GpuMemorySample {
                    id: target.id.clone(),
                    name: target.name.clone(),
                    vram_used_bytes: vram,
                    gtt_used_bytes: gtt,
                }
            })
            .collect();
        MemorySample {
            system_ram_nonavailable_bytes: ram,
            gpus,
        }
    }
}
fn nonavailable(pool: MemoryPool) -> Option<u64> {
    pool.total_bytes.checked_sub(pool.available_bytes?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn profile() -> HardwareProfile {
        HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: None,
            gpus: vec![],
            issues: vec![],
            captured_at_unix_ms: 1,
        }
    }
    fn gpu(id: &str) -> super::super::GpuDeviceProfile {
        super::super::GpuDeviceProfile {
            id: Some(id.into()),
            name: "fixture AMD".into(),
            vendor: GpuVendor::Amd,
            dedicated_vram: None,
            shared_gtt: None,
            supported_backends: vec![],
            unified_memory: None,
        }
    }
    #[test]
    fn samples_fixture_device_pools_independently_and_preserves_unknown_ram() {
        let mut profile = profile();
        profile.gpus = vec![gpu("pci:0000:03:00.0")];
        let mut probe = MemoryProbe::new(&profile);
        probe.pci_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-devices");
        let sample = probe.sample_devices(None);
        assert_eq!(sample.system_ram_nonavailable_bytes, None);
        assert_eq!(sample.gpus[0].vram_used_bytes, Some(1_073_741_824));
        assert_eq!(sample.gpus[0].gtt_used_bytes, Some(2_147_483_648));
        probe.pci_root = PathBuf::from("/this-path-does-not-exist-kvlm-sampling");
        let missing = probe.sample_devices(Some(0));
        assert_eq!(missing.system_ram_nonavailable_bytes, Some(0));
        assert_eq!(missing.gpus[0].vram_used_bytes, None);
        assert_eq!(missing.gpus[0].gtt_used_bytes, None);
    }
    #[test]
    fn rejects_unstable_duplicate_and_path_like_device_identities() {
        let mut profile = profile();
        profile.gpus = vec![
            gpu("pci:../../etc"),
            gpu("pci:0000:03:00.0"),
            gpu("pci:0000:03:00.0"),
            gpu("0000:04:00.0"),
        ];
        let probe = MemoryProbe::new(&profile);
        assert_eq!(probe.targets.len(), 1);
        assert_eq!(probe.targets[0].bus, "0000:03:00.0");
        for device in 0..32 {
            profile
                .gpus
                .push(gpu(&format!("pci:0000:04:{device:02x}.0")));
        }
        assert_eq!(MemoryProbe::new(&profile).targets.len(), MAX_GPUS);
    }
    #[test]
    fn unknown_and_inconsistent_ram_never_become_zero_samples() {
        assert_eq!(
            nonavailable(MemoryPool {
                total_bytes: 100,
                available_bytes: None
            }),
            None
        );
        assert_eq!(
            nonavailable(MemoryPool {
                total_bytes: 100,
                available_bytes: Some(101)
            }),
            None
        );
        assert_eq!(
            nonavailable(MemoryPool {
                total_bytes: 100,
                available_bytes: Some(100)
            }),
            Some(0)
        );
        assert!(MemoryProbe::new(&profile()).targets.is_empty());
    }
}
