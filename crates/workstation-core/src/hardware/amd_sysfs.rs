// SPDX-License-Identifier: GPL-3.0-or-later
//! DRM device inventory and AMD VRAM/GTT snapshots. No writes or subprocesses.

use super::{GpuDeviceProfile, GpuVendor, MemoryPool, ProbeIssue};
use std::{collections::BTreeSet, fs, path::Path};

pub(super) fn pci_id(value: &str) -> Option<String> {
    let fields: Vec<_> = value.split(':').collect();
    if fields.len() != 3 {
        return None;
    }
    let domain = u16::from_str_radix(fields[0], 16).ok()?;
    let bus = u8::from_str_radix(fields[1], 16).ok()?;
    let (device, function) = fields[2].split_once('.')?;
    let device = u8::from_str_radix(device, 16).ok()?;
    let function = u8::from_str_radix(function, 16).ok()?;
    if device > 31 || function > 7 {
        return None;
    }
    Some(format!(
        "pci:{domain:04x}:{bus:02x}:{device:02x}.{function:x}"
    ))
}

pub(super) fn probe(root: &Path) -> (Vec<GpuDeviceProfile>, Vec<ProbeIssue>) {
    let mut issues = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => return (Vec::new(), vec![ProbeIssue::new("GPU inventory", error)]),
    };
    let mut seen = BTreeSet::new();
    let mut devices = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                issues.push(ProbeIssue::new("GPU inventory", error));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_prefix("card")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let path = entry.path().join("device");
        let canonical = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(error) => {
                issues.push(ProbeIssue::new("GPU inventory", format!("{name}: {error}")));
                path.clone()
            }
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let id = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(pci_id);
        if id.is_none() {
            issues.push(ProbeIssue::new(
                "GPU inventory",
                format!("{name}: stable PCI identity is unavailable."),
            ));
        }
        let vendor = match fs::read_to_string(path.join("vendor")) {
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "0x10de" => GpuVendor::Nvidia,
                "0x1002" => GpuVendor::Amd,
                "0x8086" => GpuVendor::Intel,
                _ => GpuVendor::Other,
            },
            Err(error) => {
                issues.push(ProbeIssue::new(
                    "GPU inventory",
                    format!("{name}: vendor unavailable: {error}"),
                ));
                GpuVendor::Other
            }
        };
        let device_id =
            fs::read_to_string(path.join("device")).unwrap_or_else(|_| "unknown device".into());
        let product_name = fs::read_to_string(path.join("product_name"))
            .ok()
            .filter(|value| !value.trim().is_empty());
        let mut gpu = GpuDeviceProfile {
            id,
            name: product_name
                .map(|value| value.trim().to_owned())
                .unwrap_or_else(|| format!("{vendor:?} GPU ({})", device_id.trim())),
            vendor,
            dedicated_vram: None,
            shared_gtt: None,
            supported_backends: Vec::new(),
            unified_memory: None,
        };
        match vendor {
            GpuVendor::Amd => {
                gpu.dedicated_vram = read_pool(&path, "vram", &name, &mut issues);
                gpu.shared_gtt = read_pool(&path, "gtt", &name, &mut issues);
            }
            GpuVendor::Nvidia => {}
            _ => issues.push(ProbeIssue::new(
                &name,
                "GPU detected; this vendor's memory/backend probe is not implemented yet.",
            )),
        }
        devices.push(gpu);
    }
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    (devices, issues)
}

fn read_pool(
    path: &Path,
    kind: &str,
    name: &str,
    issues: &mut Vec<ProbeIssue>,
) -> Option<MemoryPool> {
    let read = |field| -> Result<u64, String> {
        fs::read_to_string(path.join(format!("mem_info_{kind}_{field}")))
            .map_err(|error| error.to_string())?
            .trim()
            .parse()
            .map_err(|_| "Invalid byte count.".into())
    };
    let total = match read("total") {
        Ok(value) => value,
        Err(error) => {
            issues.push(ProbeIssue::new(
                name,
                format!("{kind} total unavailable: {error}"),
            ));
            return None;
        }
    };
    match read("used").and_then(|used| MemoryPool::from_usage(total, used)) {
        Ok(memory) => Some(memory),
        Err(error) => {
            issues.push(ProbeIssue::new(
                name,
                format!("{kind} available memory unknown: {error}"),
            ));
            Some(MemoryPool {
                total_bytes: total,
                available_bytes: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm")
    }

    #[test]
    fn normalizes_nvml_and_linux_pci_domains() {
        assert_eq!(pci_id("00000000:01:00.0"), pci_id("0000:01:00.0"));
        for id in ["card0", "0000:00:20.0", "0000:00:01.8", "invalid"] {
            assert!(pci_id(id).is_none());
        }
    }

    #[test]
    fn inventories_unique_devices_and_keeps_shared_pools_separate() {
        let (devices, issues) = probe(&fixtures());
        assert_eq!(devices.len(), 2);
        let amd = devices
            .iter()
            .find(|gpu| gpu.vendor == GpuVendor::Amd)
            .unwrap();
        assert_eq!(amd.dedicated_vram.unwrap().total_bytes, 2147483648);
        assert_eq!(
            amd.dedicated_vram.unwrap().available_bytes,
            Some(1073741824)
        );
        assert_eq!(amd.shared_gtt.unwrap().total_bytes, 8589934592);
        assert_eq!(amd.unified_memory, None);
        let intel = devices
            .iter()
            .find(|gpu| gpu.vendor == GpuVendor::Intel)
            .unwrap();
        assert!(intel.dedicated_vram.is_none());
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("not implemented"))
        );
    }

    #[test]
    fn inconsistent_used_or_missing_nodes_preserve_honest_partial_data() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bad-memory");
        let mut issues = Vec::new();
        let memory = read_pool(&root, "vram", "fixture", &mut issues).unwrap();
        assert_eq!(memory.total_bytes, 100);
        assert_eq!(memory.available_bytes, None);
        assert!(read_pool(&root, "gtt", "fixture", &mut issues).is_none());
        assert_eq!(issues.len(), 2);
    }
}
