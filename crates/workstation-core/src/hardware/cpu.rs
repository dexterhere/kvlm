// SPDX-License-Identifier: GPL-3.0-or-later
//! Defensive parsing of Linux CPU and memory snapshots.

use super::{CpuFeatures, CpuProfile, MemoryPool};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn parse_cpu(input: &str) -> Result<CpuProfile, String> {
    let blocks: Vec<BTreeMap<_, _>> = input
        .split("\n\n")
        .map(|block| {
            block
                .lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.trim(), value.trim()))
                .collect()
        })
        .filter(|fields: &BTreeMap<_, _>| fields.contains_key("processor"))
        .collect();
    if blocks.is_empty() {
        return Err("No logical processors reported in /proc/cpuinfo.".into());
    }
    let mut ids = BTreeSet::new();
    let mut cores = BTreeSet::new();
    let mut topology_known = true;
    let mut models = BTreeSet::new();
    let mut common_flags: Option<BTreeSet<&str>> = None;
    let mut flags_known = true;
    for fields in &blocks {
        let id = fields["processor"]
            .parse::<u32>()
            .map_err(|_| "Invalid logical processor ID.")?;
        if !ids.insert(id) {
            return Err("Duplicate logical processor ID.".into());
        }
        match (fields.get("physical id"), fields.get("core id")) {
            (Some(package), Some(core)) => match (package.parse::<u32>(), core.parse::<u32>()) {
                (Ok(package), Ok(core)) => {
                    cores.insert((package, core));
                }
                _ => topology_known = false,
            },
            _ => topology_known = false,
        }
        if let Some(model) = fields.get("model name").filter(|name| !name.is_empty()) {
            models.insert(*model);
        }
        if let Some(flags) = fields.get("flags") {
            let flags: BTreeSet<_> = flags.split_whitespace().collect();
            common_flags = Some(match common_flags {
                Some(previous) => previous.intersection(&flags).copied().collect(),
                None => flags,
            });
        } else {
            flags_known = false;
        }
    }
    let features = if flags_known {
        common_flags.map(|flags| CpuFeatures {
            avx2: flags.contains("avx2"),
            avx512: flags.contains("avx512f"),
            vnni: flags.contains("avx_vnni") || flags.contains("avx512_vnni"),
            amx: flags.contains("amx_tile"),
        })
    } else {
        None
    };
    Ok(CpuProfile {
        model: (!models.is_empty()).then(|| models.into_iter().collect::<Vec<_>>().join(" / ")),
        logical_cores: ids.len(),
        physical_cores: topology_known.then_some(cores.len()),
        features,
    })
}

pub(super) fn parse_memory(input: &str) -> Result<MemoryPool, String> {
    let mut values = BTreeMap::new();
    for (key, value) in input.lines().filter_map(|line| line.split_once(':')) {
        if key != "MemTotal" && key != "MemAvailable" {
            continue;
        }
        let fields: Vec<_> = value.split_whitespace().collect();
        if fields.len() != 2 || fields[1] != "kB" {
            return Err(format!("{key} has an unsupported memory unit or format."));
        }
        let bytes = fields[0]
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(1024))
            .ok_or_else(|| format!("{key} is invalid or overflows a byte count."))?;
        if values.insert(key, bytes).is_some() {
            return Err(format!("Duplicate {key}."));
        }
    }
    let total = *values.get("MemTotal").ok_or("MemTotal is unavailable.")?;
    if total == 0 {
        return Err("MemTotal must be greater than zero.".into());
    }
    let available = values.get("MemAvailable").copied();
    if available.is_some_and(|free| free > total) {
        return Err("MemAvailable exceeds total RAM; refresh the snapshot.".into());
    }
    Ok(MemoryPool {
        total_bytes: total,
        available_bytes: available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const CPU: &str = include_str!("../../tests/fixtures/proc/cpuinfo");
    const MEMORY: &str = include_str!("../../tests/fixtures/proc/meminfo");

    #[test]
    fn counts_socket_core_pairs_and_intersects_flags() {
        let cpu = parse_cpu(CPU).unwrap();
        assert_eq!(cpu.logical_cores, 3);
        assert_eq!(cpu.physical_cores, Some(2));
        assert_eq!(cpu.model.as_deref(), Some("Fixture CPU"));
        let features = cpu.features.unwrap();
        assert!(features.avx2);
        assert!(!features.avx512);
    }

    #[test]
    fn unknown_topology_and_flags_are_not_fabricated() {
        let cpu = parse_cpu("processor : 0\nmodel name : Fixture CPU\n").unwrap();
        assert_eq!(cpu.physical_cores, None);
        assert_eq!(cpu.features, None);
        for input in [
            "",
            "processor : invalid\n",
            "processor : 0\n\nprocessor : 0\n",
        ] {
            assert!(parse_cpu(input).is_err());
        }
    }

    #[test]
    fn uses_memavailable_and_checked_kib_units() {
        let memory = parse_memory(MEMORY).unwrap();
        assert_eq!(memory.total_bytes, 16 * 1024 * 1024 * 1024);
        assert_eq!(memory.available_bytes, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(
            parse_memory("MemTotal: 1024 kB\n").unwrap().available_bytes,
            None
        );
    }

    #[test]
    fn rejects_bad_units_overflow_duplicates_and_impossible_memory() {
        for input in [
            "MemTotal: 1 MB",
            "MemTotal: -1 kB",
            "MemTotal: 18446744073709551615 kB",
            "MemTotal: 0 kB",
            "MemTotal: 1 kB\nMemAvailable: 2 kB",
            "MemTotal: 1 kB\nMemTotal: 2 kB",
            "MemAvailable: 1 kB",
        ] {
            assert!(parse_memory(input).is_err(), "{input}");
        }
    }
}
