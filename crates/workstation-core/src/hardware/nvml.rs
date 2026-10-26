// SPDX-License-Identifier: GPL-3.0-or-later
//! NVIDIA snapshots using the runtime NVML library, with no CUDA link dependency.

use super::{ComputeBackend, GpuDeviceProfile, GpuVendor, MemoryPool, ProbeIssue};
use nvml_wrapper::Nvml;
use std::ffi::OsStr;

pub(super) fn probe() -> Result<(Vec<GpuDeviceProfile>, Vec<ProbeIssue>), String> {
    probe_library(OsStr::new("libnvidia-ml.so.1"))
}

fn probe_library(path: &OsStr) -> Result<(Vec<GpuDeviceProfile>, Vec<ProbeIssue>), String> {
    let nvml = Nvml::builder()
        .lib_path(path)
        .init()
        .map_err(|error| format!("NVIDIA probing unavailable: {error}"))?;
    let count = nvml.device_count().map_err(|error| error.to_string())?;
    let mut devices = Vec::new();
    let mut issues = Vec::new();
    for index in 0..count {
        let device = match nvml.device_by_index(index) {
            Ok(device) => device,
            Err(error) => {
                issues.push(ProbeIssue::new(
                    "GPU inventory",
                    format!("NVIDIA device {index}: {error}"),
                ));
                continue;
            }
        };
        let id = match device
            .pci_info()
            .ok()
            .and_then(|pci| super::amd_sysfs::pci_id(&pci.bus_id))
        {
            Some(id) => id,
            None => {
                issues.push(ProbeIssue::new(
                    "GPU inventory",
                    "NVIDIA PCI identity is unavailable.",
                ));
                continue;
            }
        };
        let name = device
            .name()
            .unwrap_or_else(|_| format!("NVIDIA GPU ({id})"));
        let memory = device
            .memory_info()
            .map_err(|error| error.to_string())
            .and_then(|memory| memory_reading(memory.total, memory.used, memory.free))
            .map_err(|error| {
                issues.push(ProbeIssue::new(&name, format!("VRAM unavailable: {error}")))
            })
            .ok();
        let mut supported_backends = Vec::new();
        match (device.cuda_compute_capability(), nvml.sys_driver_version()) {
            (Ok(capability), Ok(driver_version)) if capability.major >= 0 && capability.minor >= 0 => {
                supported_backends.push(ComputeBackend::Cuda {
                    compute_capability: (capability.major, capability.minor), driver_version,
                });
            }
            _ => issues.push(ProbeIssue::new(&name, "CUDA capability/driver could not be verified; inference backend support is unknown.")),
        }
        devices.push(GpuDeviceProfile {
            id: Some(id),
            name,
            vendor: GpuVendor::Nvidia,
            dedicated_vram: memory,
            shared_gtt: None,
            supported_backends,
            unified_memory: None,
        });
    }
    Ok((devices, issues))
}

pub(super) fn memory_reading(total: u64, used: u64, free: u64) -> Result<MemoryPool, String> {
    if used.checked_add(free).is_none_or(|sum| sum > total) {
        return Err("NVML memory counters exceed total VRAM.".into());
    }
    // NVML can reserve memory; free is authoritative, not total minus used.
    Ok(MemoryPool {
        total_bytes: total,
        available_bytes: Some(free),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_library_is_recoverable_without_requiring_gpu_hardware() {
        assert!(probe_library(OsStr::new("/this-library-does-not-exist-kvlm-test.so")).is_err());
    }

    #[test]
    fn respects_reserved_memory_and_rejects_invalid_counters() {
        assert_eq!(
            memory_reading(100, 60, 30).unwrap().available_bytes,
            Some(30)
        );
        assert!(memory_reading(100, 60, 50).is_err());
        assert!(memory_reading(u64::MAX, u64::MAX, 1).is_err());
    }
}
