// SPDX-License-Identifier: GPL-3.0-or-later
//! A shareable, redacted text report of what the probes found, for hardware validation
//! records. It names models and sizes but never device identifiers (PCI addresses, UUIDs),
//! the identity hash, hostnames, paths or the capture time, and it says "not measured" for
//! anything a probe could not read rather than guessing.

use super::{ComputeBackend, GpuVendor, HardwareProfile, MemoryPool};
use std::fmt::Write;

fn gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64)
}

fn pool(pool: Option<MemoryPool>) -> String {
    match pool {
        Some(pool) => match pool.available_bytes {
            Some(free) => format!(
                "{} total, {} free at capture",
                gib(pool.total_bytes),
                gib(free)
            ),
            None => format!("{} total, free not measured", gib(pool.total_bytes)),
        },
        None => "not measured".into(),
    }
}

/// Free text from a probe is cut and stripped so a report cannot smuggle control characters.
/// Known device identifiers (in raw and normalized forms) are replaced, and so is any word
/// that looks like a path, an address, a timestamp or a UUID, because names and error
/// messages can carry them.
fn text(value: &str, secrets: &[String]) -> String {
    let mut clean: String = value.chars().filter(|c| !c.is_control()).collect();
    for secret in secrets {
        clean = clean.replace(secret.as_str(), "[redacted]");
    }
    let words: Vec<&str> = clean
        .split(' ')
        .map(|word| {
            if looks_private(word) {
                "[redacted]"
            } else {
                word
            }
        })
        .collect();
    words.join(" ").chars().take(120).collect()
}

fn looks_private(word: &str) -> bool {
    let word = word.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | '.' | '\'' | '"'));
    word.starts_with(['/', '~'])
        || word.matches('/').count() >= 2
        || word.contains(['\\', '@'])
        || (word.contains(':') && word.bytes().any(|b| b.is_ascii_digit()))
        || (word.len() >= 16
            && word
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && word.bytes().any(|b| b.is_ascii_digit()))
}

/// An identifier and the forms it may appear in elsewhere: without a `pci:` prefix, and
/// without the PCI domain.
fn forms(id: &str) -> Vec<String> {
    let bare = id.strip_prefix("pci:").unwrap_or(id);
    let short = bare.split_once(':').map_or(bare, |(_, rest)| rest);
    let mut out = vec![id.to_string(), bare.to_string()];
    if short.contains(':') {
        out.push(short.to_string());
    }
    out
}

pub fn redacted_report(profile: &HardwareProfile, app_version: &str) -> String {
    let secrets: Vec<String> = profile
        .gpus
        .iter()
        .filter_map(|gpu| gpu.id.as_deref())
        .flat_map(forms)
        .chain(profile.profile_hash.clone())
        .filter(|secret| !secret.is_empty())
        .collect();
    let text = |value: &str| text(value, &secrets);
    let mut out = String::new();
    let _ = writeln!(out, "Local AI Workstation hardware report");
    let _ = writeln!(out, "Application version: {}", text(app_version));
    let _ = writeln!(out, "Architecture: {}", std::env::consts::ARCH);
    match &profile.cpu {
        Some(cpu) => {
            let _ = writeln!(
                out,
                "CPU: {} ({} physical, {} logical cores)",
                cpu.model
                    .as_deref()
                    .map_or_else(|| "not measured".to_string(), text),
                cpu.physical_cores
                    .map_or("unknown".to_string(), |n| n.to_string()),
                cpu.logical_cores
            );
            let _ = writeln!(
                out,
                "CPU features: {}",
                match cpu.features {
                    Some(f) => format!(
                        "AVX2 {}, AVX-512 {}, VNNI {}, AMX {}",
                        f.avx2, f.avx512, f.vnni, f.amx
                    ),
                    None => "not measured".into(),
                }
            );
        }
        None => {
            let _ = writeln!(out, "CPU: not measured");
        }
    }
    let _ = writeln!(out, "System RAM: {}", pool(profile.system_ram));
    let _ = writeln!(out, "GPUs: {}", profile.gpus.len());
    for (index, gpu) in profile.gpus.iter().enumerate() {
        let vendor = match gpu.vendor {
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::Other => "other",
        };
        let _ = writeln!(out, "GPU {}: {} ({vendor})", index + 1, text(&gpu.name));
        let _ = writeln!(out, "  Dedicated VRAM: {}", pool(gpu.dedicated_vram));
        let _ = writeln!(out, "  Shared GPU memory (GTT): {}", pool(gpu.shared_gtt));
        let _ = writeln!(
            out,
            "  Unified memory: {}",
            gpu.unified_memory
                .map_or("not measured", |u| if u { "yes" } else { "no" })
        );
        if gpu.supported_backends.is_empty() {
            let _ = writeln!(out, "  Compute backends: none detected");
        }
        for backend in &gpu.supported_backends {
            let ComputeBackend::Cuda {
                compute_capability: (major, minor),
                driver_version,
            } = backend;
            let _ = writeln!(
                out,
                "  Compute backend: CUDA compute capability {major}.{minor}, driver {}",
                text(driver_version)
            );
        }
    }
    if profile.issues.is_empty() {
        let _ = writeln!(out, "Probe notices: none");
    } else {
        let _ = writeln!(out, "Probe notices:");
        for issue in &profile.issues {
            let _ = writeln!(
                out,
                "  {}: {}",
                text(&issue.component),
                text(&issue.message)
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_report_names_hardware_but_no_identifier_hash_or_time() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut profile = HardwareProfile::detect_linux(&root.join("proc"), &root.join("drm"));
        profile.update_hash();
        let ids: Vec<String> = profile.gpus.iter().filter_map(|g| g.id.clone()).collect();
        let hash = profile
            .profile_hash
            .clone()
            .unwrap_or_else(|| "no-hash".into());
        let report = redacted_report(&profile, "0.1.0");
        assert!(report.contains("System RAM:") && report.contains("CPU:"));
        for id in ids {
            assert!(!report.contains(&id), "device id leaked");
        }
        assert!(!report.contains(&hash));
        assert!(!report.contains(&profile.captured_at_unix_ms.to_string()));
    }

    #[test]
    fn an_identifier_inside_a_name_or_notice_is_removed() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut profile = HardwareProfile::detect_linux(&root.join("proc"), &root.join("drm"));
        profile.gpus.truncate(1);
        let mut gpu = profile.gpus.remove(0);
        gpu.id = Some("pci:0000:03:00.0".into());
        gpu.name = "Some GPU 0000:03:00.0".into();
        profile.gpus.push(gpu);
        profile.issues.push(super::super::ProbeIssue::new(
            "GPU",
            "failed on 0000:03:00.0",
        ));
        let report = redacted_report(&profile, "0.1.0");
        assert!(!report.contains("03:00.0"), "{report}");
        assert!(report.contains("Some GPU [redacted]"));
    }

    #[test]
    fn paths_uuids_and_addresses_in_notices_are_removed() {
        let mut profile =
            HardwareProfile::detect_linux(Path::new("/nonexistent"), Path::new("/nonexistent"));
        profile.issues.push(super::super::ProbeIssue::new(
            "NVIDIA NVML",
            "cannot load /home/alice/driver/libnvidia-ml.so.1 for GPU-3b1e4c3a-9f0d-4c1e-8a77-0123456789ab at 12:30:45 on box@lan",
        ));
        let report = redacted_report(&profile, "0.1.0");
        for leak in ["alice", "3b1e4c3a", "12:30", "box@lan"] {
            assert!(!report.contains(leak), "{leak} leaked: {report}");
        }
        assert!(report.contains("cannot load"));
    }

    #[test]
    fn missing_readings_say_not_measured_and_control_characters_are_dropped() {
        let profile = HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: None,
            gpus: vec![],
            issues: vec![super::super::ProbeIssue::new("CPU\u{1b}", "bad\nline")],
            captured_at_unix_ms: 0,
        };
        let report = redacted_report(&profile, "0.1.0");
        assert!(report.contains("CPU: not measured"));
        assert!(report.contains("System RAM: not measured"));
        assert!(!report.contains('\u{1b}') && report.contains("badline"));
    }
}
