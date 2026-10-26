// SPDX-License-Identifier: GPL-3.0-or-later
//! Blocking local-file inspection and fresh CPU/GPU budgets for desktop scenarios.

use std::{path::PathBuf, time::SystemTime};
use workstation_core::{
    fit::{
        gguf_fit::{GgufFitAssessment, LayoutAssumption, RuntimeScenario, assess_gguf_fit},
        memory_math::{ExecutionTarget, GpuBackend, KvCacheType, RuntimeConfig},
    },
    hardware::{ComputeBackend, HardwareProfile},
    models::gguf::{GgufMetadata, inspect_file},
};

#[derive(Debug)]
pub struct FitRequest {
    pub id: u64,
    pub path: PathBuf,
    pub scenario: RuntimeScenario,
    pub layout: LayoutAssumption,
}

#[derive(Debug)]
pub struct FitReport {
    pub metadata: GgufMetadata,
    pub estimate: GgufFitAssessment,
    pub available_ram_bytes: Option<u64>,
    pub captured_at_unix_ms: u64,
    pub ram_notice: Option<String>,
    pub gpu_estimates: Vec<GpuFitReport>,
    pub gpu_notices: Vec<String>,
}

#[derive(Debug)]
pub struct GpuFitReport {
    pub id: String,
    pub name: String,
    pub available_vram_bytes: Option<u64>,
    pub estimate: GgufFitAssessment,
}

pub fn parse_scenario(
    context: &str,
    microbatch: &str,
    key: KvCacheType,
    value: KvCacheType,
) -> Result<RuntimeScenario, &'static str> {
    let positive = |text: &str| text.trim().parse::<u64>().ok().filter(|value| *value > 0);
    Ok(RuntimeScenario {
        config: RuntimeConfig {
            context_tokens: positive(context).ok_or("Context must be a positive whole number.")?,
            micro_batch_tokens: positive(microbatch)
                .ok_or("Microbatch must be a positive whole number.")?,
            key_cache: key,
            value_cache: value,
        },
        sequence_count: Some(1),
    })
}

/// Runs entirely in the bridge's blocking worker, never on the GTK thread.
pub fn estimate(request: FitRequest) -> Result<FitReport, String> {
    estimate_with_hardware(request, HardwareProfile::detect)
}

fn estimate_with_hardware(
    request: FitRequest,
    hardware: impl FnOnce() -> HardwareProfile,
) -> Result<FitReport, String> {
    let metadata = inspect_file(&request.path).map_err(|error| error.to_string())?;
    fit_report(metadata, &request.scenario, request.layout, hardware())
}

/// Fit of already-inspected metadata against a fresh hardware reading.
pub fn fit_report(
    metadata: GgufMetadata,
    scenario: &RuntimeScenario,
    layout: LayoutAssumption,
    hardware: HardwareProfile,
) -> Result<FitReport, String> {
    let available_ram_bytes = hardware.system_ram.and_then(|pool| pool.available_bytes);
    let ram_notice = available_ram_bytes
        .is_none()
        .then(|| "MemAvailable was not reported; the CPU budget is unknown.".into());
    let mut gpu_estimates = Vec::new();
    let mut gpu_notices = Vec::new();
    for gpu in &hardware.gpus {
        let Some(id) = &gpu.id else {
            gpu_notices.push(format!("{}: device identity unavailable.", gpu.name));
            continue;
        };
        if !gpu
            .supported_backends
            .iter()
            .any(|backend| matches!(backend, ComputeBackend::Cuda { .. }))
        {
            gpu_notices.push(format!(
                "{}: no confirmed CUDA backend; GPU estimate unavailable.",
                gpu.name
            ));
            continue;
        }
        let available_vram_bytes = gpu.dedicated_vram.and_then(|pool| pool.available_bytes);
        let estimate = assess_gguf_fit(
            &metadata,
            scenario,
            &ExecutionTarget::DedicatedGpu {
                backend: GpuBackend::Cuda,
                available_vram_bytes,
            },
            layout,
        )
        .map_err(|error| error.to_string())?;
        gpu_estimates.push(GpuFitReport {
            id: id.clone(),
            name: gpu.name.clone(),
            available_vram_bytes,
            estimate,
        });
    }
    gpu_notices.extend(
        hardware
            .issues
            .iter()
            .filter(|issue| issue.component != "CPU" && issue.component != "System RAM")
            .map(|issue| format!("{}: {}", issue.component, issue.message)),
    );
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let estimate = assess_gguf_fit(
        &metadata,
        scenario,
        &ExecutionTarget::Cpu {
            available_ram_bytes,
        },
        layout,
    )
    .map_err(|error| error.to_string())?;
    Ok(FitReport {
        metadata,
        estimate,
        available_ram_bytes,
        captured_at_unix_ms,
        ram_notice,
        gpu_estimates,
        gpu_notices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use workstation_core::hardware::MemoryPool;

    fn profile(available: Option<u64>) -> HardwareProfile {
        HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: Some(MemoryPool {
                total_bytes: 4096,
                available_bytes: available,
            }),
            gpus: vec![],
            issues: vec![],
            captured_at_unix_ms: 1,
        }
    }
    use workstation_core::fit::gguf_fit::GgufFitStatus;

    fn request(path: PathBuf) -> FitRequest {
        FitRequest {
            id: 1,
            path,
            scenario: parse_scenario("4096", "512", KvCacheType::F16, KvCacheType::Q8_0).unwrap(),
            layout: LayoutAssumption::Unconfirmed,
        }
    }

    #[test]
    fn validates_visible_scenario_values_without_model_defaults() {
        for invalid in ["", "0", "-1", "1.5", "invalid", "18446744073709551616"] {
            assert!(parse_scenario(invalid, "512", KvCacheType::F16, KvCacheType::F16).is_err());
            assert!(parse_scenario("4096", invalid, KvCacheType::F16, KvCacheType::F16).is_err());
        }
        let scenario = parse_scenario(" 257 ", "1", KvCacheType::Q8_0, KvCacheType::Q4_0).unwrap();
        assert_eq!(scenario.config.context_tokens, 257);
        assert_eq!(scenario.config.micro_batch_tokens, 1);
        assert_eq!(scenario.config.key_cache, KvCacheType::Q8_0);
        assert_eq!(scenario.config.value_cache, KvCacheType::Q4_0);
        assert_eq!(scenario.sequence_count, Some(1));
    }

    #[test]
    fn each_estimate_reads_available_ram_and_preserves_missing_or_zero() {
        let path = std::env::temp_dir().join(format!(
            "kvlm-desktop-{}-{}.gguf",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut header = b"GGUF".to_vec();
        header.extend(3_u32.to_le_bytes());
        header.extend(0_u64.to_le_bytes());
        header.extend(0_u64.to_le_bytes());
        fs::write(&path, header).unwrap();
        for available in [Some(1234), Some(0), None] {
            let report = estimate_with_hardware(request(path.clone()), || {
                use workstation_core::hardware::{GpuDeviceProfile, GpuVendor};
                let mut profile = profile(available);
                let cuda = GpuDeviceProfile {
                    id: Some("pci:0000:01:00.0".into()),
                    name: "Fixture CUDA".into(),
                    vendor: GpuVendor::Nvidia,
                    dedicated_vram: Some(MemoryPool {
                        total_bytes: 4096,
                        available_bytes: available,
                    }),
                    shared_gtt: None,
                    unified_memory: Some(false),
                    supported_backends: vec![ComputeBackend::Cuda {
                        compute_capability: (8, 9),
                        driver_version: "fixture".into(),
                    }],
                };
                let mut unsupported = cuda.clone();
                unsupported.supported_backends.clear();
                unsupported.id = Some("pci:0000:02:00.0".into());
                let mut unidentified = cuda.clone();
                unidentified.id = None;
                profile.gpus = vec![cuda, unsupported, unidentified];
                profile
            })
            .unwrap();
            assert_eq!(report.available_ram_bytes, available);
            assert_eq!(report.ram_notice.is_some(), available.is_none());
            assert_eq!(report.estimate.status, GgufFitStatus::NoTensorData);
            assert_eq!(report.gpu_estimates.len(), 1);
            assert_eq!(report.gpu_estimates[0].available_vram_bytes, available);
            assert_eq!(
                report.gpu_estimates[0].estimate.assessment.memory_class(),
                None
            );
            assert_eq!(report.gpu_notices.len(), 2);
            assert!(report.captured_at_unix_ms > 0);
        }
        let report = estimate_with_hardware(request(path.clone()), || profile(None)).unwrap();
        assert_eq!(report.available_ram_bytes, None);
        assert!(report.ram_notice.is_some());
        fs::write(&path, b"bad file").unwrap();
        assert!(
            estimate_with_hardware(request(path.clone()), || panic!(
                "Do not probe RAM for invalid files"
            ))
            .is_err()
        );
        fs::remove_file(path).unwrap();
    }
}
