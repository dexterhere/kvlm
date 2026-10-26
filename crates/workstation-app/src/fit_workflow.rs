// SPDX-License-Identifier: GPL-3.0-or-later
//! Blocking local-file inspection and fresh CPU budget for desktop scenarios.

use std::{path::PathBuf, time::SystemTime};
use workstation_core::{
    fit::{
        gguf_fit::{GgufFitAssessment, LayoutAssumption, RuntimeScenario, assess_gguf_fit},
        memory_math::{ExecutionTarget, KvCacheType, RuntimeConfig},
    },
    hardware::{MemoryPool, cpu::read_memory},
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
    estimate_with_memory(request, read_memory)
}

fn estimate_with_memory(
    request: FitRequest,
    memory: impl FnOnce() -> Result<MemoryPool, String>,
) -> Result<FitReport, String> {
    let metadata = inspect_file(&request.path).map_err(|error| error.to_string())?;
    let (available_ram_bytes, ram_notice) = match memory() {
        Ok(pool) => (
            pool.available_bytes,
            pool.available_bytes
                .is_none()
                .then(|| "MemAvailable was not reported; the CPU budget is unknown.".into()),
        ),
        Err(error) => (None, Some(error)),
    };
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let estimate = assess_gguf_fit(
        &metadata,
        &request.scenario,
        &ExecutionTarget::Cpu {
            available_ram_bytes,
        },
        request.layout,
    )
    .map_err(|error| error.to_string())?;
    Ok(FitReport {
        metadata,
        estimate,
        available_ram_bytes,
        captured_at_unix_ms,
        ram_notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
            let report = estimate_with_memory(request(path.clone()), || {
                Ok(MemoryPool {
                    total_bytes: 4096,
                    available_bytes: available,
                })
            })
            .unwrap();
            assert_eq!(report.available_ram_bytes, available);
            assert_eq!(report.ram_notice.is_some(), available.is_none());
            assert_eq!(report.estimate.status, GgufFitStatus::NoTensorData);
            assert!(report.captured_at_unix_ms > 0);
        }
        let report =
            estimate_with_memory(request(path.clone()), || Err("Fixture RAM failure".into()))
                .unwrap();
        assert_eq!(report.available_ram_bytes, None);
        assert_eq!(report.ram_notice.as_deref(), Some("Fixture RAM failure"));
        fs::write(&path, b"bad file").unwrap();
        assert!(
            estimate_with_memory(request(path.clone()), || panic!(
                "Do not probe RAM for invalid files"
            ))
            .is_err()
        );
        fs::remove_file(path).unwrap();
    }
}
