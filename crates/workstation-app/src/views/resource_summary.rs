// SPDX-License-Identifier: GPL-3.0-or-later

use workstation_core::hardware::resources::ResourceSummary;

const MAX_TEXT: usize = 1024;
const MAX_GPU_NAME: usize = 120;

pub(super) fn fields(resources: Option<&ResourceSummary>) -> Vec<String> {
    let Some(resources) = resources else {
        return vec!["Memory sampling: Not sampled".into()];
    };

    let mut fields = vec![format!(
        "System RAM nonavailable (system-wide peak): {} · {} samples",
        bytes(resources.peak_system_ram_nonavailable_bytes),
        resources.system_ram_samples
    )];
    if resources.gpus.is_empty() {
        fields.push("GPU memory: Unknown (no device readings collected)".into());
    } else {
        for (index, gpu) in resources.gpus.iter().take(16).enumerate() {
            let name = bounded(&gpu.name, MAX_GPU_NAME);
            fields.push(format!(
                "GPU {} ({name}) driver-reported VRAM peak: {} · {} samples",
                index + 1,
                bytes(gpu.peak_vram_used_bytes),
                gpu.vram_samples
            ));
            fields.push(format!(
                "GPU {} ({name}) GTT peak (overlaps system RAM): {} · {} samples",
                index + 1,
                bytes(gpu.peak_gtt_used_bytes),
                gpu.gtt_samples
            ));
        }
        if resources.gpus.len() > 16 {
            fields.push("GPU readings: additional devices omitted".into());
        }
    }
    fields.push(format!(
        "Sampling: {} total samples · {} ms interval · {} ms window; system/device-wide observations during runtime checks and generation (excludes hardware discovery and history writes), 100 ms target interval; lower-bound peaks, not process footprint or proven true peak",
        resources.samples, resources.interval_ms, resources.window_ms
    ));
    if resources.system_ram_samples < resources.samples
        || resources
            .gpus
            .iter()
            .any(|gpu| gpu.vram_samples < resources.samples || gpu.gtt_samples < resources.samples)
    {
        fields.push("Coverage: some channels have missing or unsupported readings; peaks include only valid observations".into());
    }
    if resources.probe_failed {
        fields.push("Coverage: a blocking probe worker failed; readings may be partial".into());
    }
    if resources.probe_in_flight {
        fields.push(
            "Coverage: the last probe was not collected before the run ended; readings may be partial"
                .into(),
        );
    }
    if resources.limit_reached {
        fields.push("Coverage: sampling limit reached; readings may be partial".into());
    }
    fields
        .into_iter()
        .map(|field| bounded(&field, MAX_TEXT))
        .collect()
}

fn bytes(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value} bytes"))
        .unwrap_or_else(|| "Unknown".into())
}

fn bounded(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{prefix}… (truncated)")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workstation_core::hardware::resources::GpuMemoryPeak;
    #[test]
    fn legacy_missing_and_zero_readings_remain_distinct() {
        assert_eq!(fields(None), vec!["Memory sampling: Not sampled"]);
        let missing = fields(Some(&ResourceSummary::default())).join("\n");
        assert!(missing.contains("Unknown"));
        assert!(!missing.contains("CPU-only"));
        let summary = ResourceSummary {
            samples: 2,
            system_ram_samples: 1,
            peak_system_ram_nonavailable_bytes: Some(0),
            gpus: vec![GpuMemoryPeak {
                id: "fixture".into(),
                name: "long name".repeat(1000),
                vram_samples: 1,
                peak_vram_used_bytes: Some(0),
                gtt_samples: 0,
                peak_gtt_used_bytes: None,
            }],
            ..ResourceSummary::default()
        };
        let text = fields(Some(&summary)).join("\n");
        assert!(text.contains("peak): 0 bytes"));
        assert!(text.contains("VRAM peak: 0 bytes"));
        assert!(text.contains("GTT peak (overlaps system RAM): Unknown"));
        assert!(text.contains("missing or unsupported readings"));
        assert!(text.contains("not process footprint"));
        assert!(
            fields(Some(&summary))
                .iter()
                .all(|s| s.chars().count() < MAX_TEXT)
        );
    }
}
