// SPDX-License-Identifier: GPL-3.0-or-later
//! Utilization readings for one benchmark pass: system CPU, the processes this application
//! owns, and GPU busy percentage. Read-only (procfs, sysfs, NVML); call `sample` only from a
//! blocking worker. A reading that cannot be taken is `None`, never zero.

use super::{
    GpuVendor, HardwareProfile,
    resources::{Target, targets},
};
use crate::experiments::{Coverage, PassMetrics, Sampled, Scope};
use nvml_wrapper::Nvml;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

/// `/proc` reports CPU time in USER_HZ units, which is 100 on every Linux architecture.
pub const TICKS_PER_SECOND: u64 = 100;
/// Entries examined per scan for members of the owned process group.
const MAX_SCAN: usize = 16_384;
/// Samples kept per pass; later ones are dropped and the result says so.
pub const MAX_LOAD_SAMPLES: u32 = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuTicks {
    pub busy: u64,
    pub total: u64,
}

/// Summed over every live process of the owned group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupUsage {
    pub processes: u32,
    pub rss_bytes: u64,
    pub cpu_ticks: u64,
}

pub fn parse_cpu_stat(text: &str) -> Option<CpuTicks> {
    let line = text.lines().next()?;
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal; guest time is already inside user.
    let values: Vec<u64> = fields
        .take(8)
        .map(|f| f.parse().ok())
        .collect::<Option<_>>()?;
    if values.len() < 8 {
        return None;
    }
    let total = values.iter().try_fold(0u64, |a, v| a.checked_add(*v))?;
    let idle = values[3].checked_add(values[4])?;
    Some(CpuTicks {
        busy: total.checked_sub(idle)?,
        total,
    })
}

struct PidStat {
    pgrp: i32,
    cpu_ticks: u64,
    rss_pages: u64,
}

fn parse_pid_stat(text: &str) -> Option<PidStat> {
    // The command name is parenthesised and may hold spaces or parentheses.
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // fields[0] is field 3 (state); field n is fields[n - 3].
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let rss: i64 = fields.get(21)?.parse().ok()?;
    Some(PidStat {
        pgrp: fields.get(2)?.parse().ok()?,
        cpu_ticks: utime.checked_add(stime)?,
        rss_pages: u64::try_from(rss).ok()?,
    })
}

pub fn read_cpu(proc_root: &Path) -> Option<CpuTicks> {
    parse_cpu_stat(&fs::read_to_string(proc_root.join("stat")).ok()?)
}

/// Usage of every process in `pgid`; `None` when the group has no live member, so a
/// stopped runtime reads as unknown rather than as an idle zero.
pub fn read_group(proc_root: &Path, pgid: i32, page_size: u64) -> Option<GroupUsage> {
    let mut usage = GroupUsage::default();
    let mut scanned = 0usize;
    for entry in fs::read_dir(proc_root).ok()?.flatten() {
        scanned += 1;
        if scanned > MAX_SCAN {
            // Members may have been missed, so any total would be a silent undercount.
            return None;
        }
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        // A process may exit between listing and reading; that is not an error.
        let Ok(bytes) = fs::read(proc_root.join(pid).join("stat")) else {
            continue;
        };
        // A command name may hold arbitrary bytes; only the numeric fields matter.
        let text = String::from_utf8_lossy(&bytes);
        let Some(stat) = parse_pid_stat(&text).filter(|s| s.pgrp == pgid) else {
            continue;
        };
        usage.processes += 1;
        usage.rss_bytes = usage
            .rss_bytes
            .saturating_add(stat.rss_pages.saturating_mul(page_size));
        usage.cpu_ticks = usage.cpu_ticks.saturating_add(stat.cpu_ticks);
    }
    (usage.processes > 0).then_some(usage)
}

fn amd_busy_percent(pci_root: &Path, bus: &str) -> Option<u8> {
    let text = fs::read_to_string(pci_root.join(bus).join("gpu_busy_percent")).ok()?;
    text.trim().parse::<u8>().ok().filter(|v| *v <= 100)
}

#[derive(Clone, Debug)]
pub struct LoadSample {
    pub at: Instant,
    pub cpu: Option<CpuTicks>,
    pub group: Option<GroupUsage>,
    /// `(stable device id, busy percent)`
    pub gpus: Vec<(String, Option<u8>)>,
}

pub struct LoadProbe {
    proc_root: PathBuf,
    pci_root: PathBuf,
    pgid: Option<i32>,
    page_size: u64,
    targets: Vec<Target>,
    nvml: Option<Result<Nvml, ()>>,
}

impl LoadProbe {
    /// `owned_pgid` is the process group of a Managed runtime; `None` for an Attached one,
    /// whose process this application neither owns nor measures.
    pub fn new(profile: &HardwareProfile, owned_pgid: Option<i32>) -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            pci_root: PathBuf::from("/sys/bus/pci/devices"),
            pgid: owned_pgid.filter(|p| *p > 1),
            page_size: rustix::param::page_size() as u64,
            targets: targets(profile),
            nvml: None,
        }
    }

    pub fn sample(&mut self) -> LoadSample {
        if self.nvml.is_none() && self.targets.iter().any(|t| t.vendor == GpuVendor::Nvidia) {
            self.nvml = Some(Nvml::init().map_err(|_| ()));
        }
        let gpus = self
            .targets
            .iter()
            .map(|target| {
                let busy = match target.vendor {
                    GpuVendor::Amd => amd_busy_percent(&self.pci_root, &target.bus),
                    GpuVendor::Nvidia => self
                        .nvml
                        .as_ref()
                        .and_then(|library| library.as_ref().ok())
                        .and_then(|library| library.device_by_pci_bus_id(target.bus.as_str()).ok())
                        .and_then(|device| device.utilization_rates().ok())
                        .map(|rates| rates.gpu.min(100) as u8),
                    _ => None,
                };
                (target.id.clone(), busy)
            })
            .collect();
        LoadSample {
            at: Instant::now(),
            cpu: read_cpu(&self.proc_root),
            group: self
                .pgid
                .and_then(|pgid| read_group(&self.proc_root, pgid, self.page_size)),
            gpus,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Series {
    count: u32,
    peak: f64,
    sum: f64,
    first_at: Option<Instant>,
    last_at: Option<Instant>,
    longest_gap_ms: u64,
}

impl Series {
    fn add(&mut self, at: Instant, value: f64) {
        if let Some(last) = self.last_at {
            let gap = at.saturating_duration_since(last).as_millis();
            self.longest_gap_ms = self.longest_gap_ms.max(gap.min(u64::MAX as u128) as u64);
        }
        self.first_at.get_or_insert(at);
        self.last_at = Some(at);
        self.count += 1;
        self.peak = if self.count == 1 {
            value
        } else {
            self.peak.max(value)
        };
        self.sum += value;
    }

    fn finish(&self, scope: Scope, capped: bool) -> Option<Sampled> {
        if self.count == 0 {
            return None;
        }
        let covered = self.last_at?.saturating_duration_since(self.first_at?);
        Some(Sampled {
            peak: self.peak,
            mean: self.sum / f64::from(self.count),
            coverage: Coverage {
                scope,
                samples: self.count,
                covered_ms: covered.as_millis().min(u64::MAX as u128) as u64,
                longest_gap_ms: self.longest_gap_ms,
                capped,
            },
        })
    }
}

/// Folds the samples of one pass. CPU percentages come from the difference between two
/// consecutive samples, so the first sample only sets the baseline.
#[derive(Debug, Default)]
pub struct LoadAggregate {
    previous: Option<LoadSample>,
    first_group: Option<GroupUsage>,
    last_group: Option<GroupUsage>,
    cpu: Series,
    rss: Series,
    gpus: Vec<(String, Series)>,
    seen: u32,
    capped: bool,
}

impl LoadAggregate {
    pub fn add(&mut self, sample: LoadSample) {
        if self.seen >= MAX_LOAD_SAMPLES {
            self.capped = true;
            return;
        }
        self.seen += 1;
        if let (Some(before), Some(now)) = (self.previous.as_ref().and_then(|p| p.cpu), sample.cpu)
            && let (Some(total), Some(busy)) = (
                now.total.checked_sub(before.total).filter(|t| *t > 0),
                now.busy.checked_sub(before.busy),
            )
            // iowait may legitimately go backwards, which shrinks idle time; such an interval
            // is skipped instead of being turned into a utilization figure.
            && now.total - now.busy >= before.total - before.busy
        {
            self.cpu.add(
                sample.at,
                (busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0),
            );
        }
        if let Some(group) = sample.group {
            self.rss.add(sample.at, group.rss_bytes as f64);
            self.first_group.get_or_insert(group);
            self.last_group = Some(group);
        }
        for (id, busy) in &sample.gpus {
            let Some(busy) = busy else { continue };
            let index = match self.gpus.iter().position(|(known, _)| known == id) {
                Some(i) => i,
                None => {
                    self.gpus.push((id.clone(), Series::default()));
                    self.gpus.len() - 1
                }
            };
            self.gpus[index].1.add(sample.at, f64::from(*busy));
        }
        self.previous = Some(sample);
    }

    /// Write what was measured into `metrics`; anything not measured is left as it was.
    pub fn apply(&self, metrics: &mut PassMetrics) {
        metrics.cpu_utilization_percent = self.cpu.finish(Scope::System, self.capped);
        metrics.process_rss_bytes = self.rss.finish(Scope::OwnedProcessGroup, self.capped);
        // One device is reported: the one that worked hardest, which is what a "GPU is idle"
        // rule must not be fooled by on a multi-GPU machine.
        metrics.gpu_utilization_percent = self
            .gpus
            .iter()
            .filter_map(|(_, s)| s.finish(Scope::Device, self.capped))
            .max_by(|a, b| a.peak.total_cmp(&b.peak));
        if let (Some(first), Some(last)) = (self.first_group, self.last_group) {
            // A process that exited mid-pass takes its time with it, so this can undercount.
            let ticks = last.cpu_ticks.saturating_sub(first.cpu_ticks);
            metrics.process_cpu_ms = Some(ticks.saturating_mul(1000 / TICKS_PER_SECOND));
        }
    }
}

#[cfg(test)]
mod tests;
