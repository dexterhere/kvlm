use super::*;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kvlm-load-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn proc(&self, pid: &str, stat: &str) {
        fs::create_dir_all(self.0.join(pid)).unwrap();
        fs::write(self.0.join(pid).join("stat"), stat).unwrap();
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stat_line(comm: &str, pgrp: i32, utime: u64, stime: u64, rss: i64) -> String {
    // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt
    // utime stime cutime cstime priority nice threads itrealvalue starttime vsize rss
    format!("42 ({comm}) S 1 {pgrp} 1 0 -1 0 0 0 0 0 {utime} {stime} 0 0 20 0 1 0 100 4096 {rss} 0")
}

#[test]
fn cpu_stat_excludes_idle_and_iowait_and_rejects_garbage() {
    let ticks = parse_cpu_stat("cpu  100 0 50 800 40 5 5 0 0 0\ncpu0 1 1 1 1 1 1 1 1\n").unwrap();
    assert_eq!((ticks.busy, ticks.total), (160, 1000));
    assert!(parse_cpu_stat("cpu0 1 2 3").is_none());
    assert!(parse_cpu_stat("cpu 1 2 3 4").is_none());
    assert!(parse_cpu_stat("cpu 1 2 x 4 5 6 7 8").is_none());
    assert!(parse_cpu_stat("").is_none());
}

#[test]
fn pid_stat_survives_spaces_and_parentheses_in_the_command_name() {
    let stat = parse_pid_stat(&stat_line("llama (server) x", 77, 30, 12, 250)).unwrap();
    assert_eq!((stat.pgrp, stat.cpu_ticks, stat.rss_pages), (77, 42, 250));
    assert!(parse_pid_stat("42 (short) S 1").is_none());
    assert!(parse_pid_stat(&stat_line("x", 1, 0, 0, -5)).is_none());
}

#[test]
fn group_usage_sums_only_the_owned_group_and_is_unknown_when_it_is_gone() {
    let dir = Dir::new();
    dir.proc("100", &stat_line("llama-server", 100, 200, 50, 1000));
    dir.proc("101", &stat_line("worker", 100, 10, 5, 24));
    dir.proc("200", &stat_line("other", 200, 999, 999, 999));
    dir.proc("self", "not a pid directory");
    fs::write(dir.0.join("meminfo"), "x").unwrap();
    let usage = read_group(&dir.0, 100, 4096).unwrap();
    assert_eq!(usage.processes, 2);
    assert_eq!(usage.rss_bytes, 1024 * 4096);
    assert_eq!(usage.cpu_ticks, 265);
    assert!(
        read_group(&dir.0, 300, 4096).is_none(),
        "no member is not zero usage"
    );
    assert!(read_group(&dir.0.join("missing"), 100, 4096).is_none());
}

#[test]
fn reads_this_machine_without_failing() {
    let cpu = read_cpu(Path::new("/proc")).unwrap();
    assert!(cpu.total >= cpu.busy);
    let own_group = rustix::process::getpgid(None)
        .unwrap()
        .as_raw_nonzero()
        .get();
    let usage = read_group(
        Path::new("/proc"),
        own_group,
        rustix::param::page_size() as u64,
    )
    .unwrap();
    assert!(usage.processes >= 1 && usage.rss_bytes > 0);
}

#[test]
fn amd_busy_percent_must_be_a_percentage() {
    let dir = Dir::new();
    let device = dir.0.join("0000:03:00.0");
    fs::create_dir_all(&device).unwrap();
    fs::write(device.join("gpu_busy_percent"), "37\n").unwrap();
    assert_eq!(amd_busy_percent(&dir.0, "0000:03:00.0"), Some(37));
    fs::write(device.join("gpu_busy_percent"), "400\n").unwrap();
    assert_eq!(amd_busy_percent(&dir.0, "0000:03:00.0"), None);
    fs::write(device.join("gpu_busy_percent"), "n/a").unwrap();
    assert_eq!(amd_busy_percent(&dir.0, "0000:03:00.0"), None);
    assert_eq!(amd_busy_percent(&dir.0, "0000:09:00.0"), None);
}

fn sample(
    at: Instant,
    cpu: Option<(u64, u64)>,
    group: Option<(u64, u64)>,
    gpus: &[(&str, Option<u8>)],
) -> LoadSample {
    LoadSample {
        at,
        cpu: cpu.map(|(busy, total)| CpuTicks { busy, total }),
        group: group.map(|(rss_bytes, cpu_ticks)| GroupUsage {
            processes: 1,
            rss_bytes,
            cpu_ticks,
        }),
        gpus: gpus.iter().map(|(id, v)| (id.to_string(), *v)).collect(),
    }
}

#[test]
fn aggregate_reports_peaks_means_coverage_and_process_cpu() {
    let t0 = Instant::now();
    let at = |ms| t0 + Duration::from_millis(ms);
    let mut load = LoadAggregate::default();
    load.add(sample(
        at(0),
        Some((0, 100)),
        Some((1000, 10)),
        &[("pci:a", Some(10))],
    ));
    load.add(sample(
        at(100),
        Some((50, 200)),
        Some((3000, 20)),
        &[("pci:a", Some(90))],
    ));
    load.add(sample(
        at(350),
        Some((100, 300)),
        Some((2000, 60)),
        &[("pci:a", None)],
    ));
    let mut metrics = PassMetrics::default();
    load.apply(&mut metrics);

    let cpu = metrics.cpu_utilization_percent.unwrap();
    assert_eq!((cpu.peak, cpu.mean, cpu.coverage.samples), (50.0, 50.0, 2));
    assert_eq!(cpu.coverage.scope, Scope::System);
    assert_eq!(cpu.coverage.longest_gap_ms, 250);
    let rss = metrics.process_rss_bytes.unwrap();
    assert_eq!((rss.peak, rss.coverage.samples), (3000.0, 3));
    assert_eq!(rss.coverage.scope, Scope::OwnedProcessGroup);
    assert_eq!(rss.coverage.covered_ms, 350);
    assert_eq!(metrics.process_cpu_ms, Some(500));
    let gpu = metrics.gpu_utilization_percent.unwrap();
    assert_eq!((gpu.peak, gpu.mean, gpu.coverage.samples), (90.0, 50.0, 2));
    assert_eq!(gpu.coverage.scope, Scope::Device);
}

#[test]
fn nothing_measured_stays_unknown_and_the_busiest_gpu_is_reported() {
    let t0 = Instant::now();
    let mut load = LoadAggregate::default();
    load.add(sample(t0, None, None, &[("pci:a", None)]));
    let mut metrics = PassMetrics::default();
    load.apply(&mut metrics);
    assert!(metrics.cpu_utilization_percent.is_none());
    assert!(metrics.process_rss_bytes.is_none());
    assert!(metrics.gpu_utilization_percent.is_none());
    assert!(metrics.process_cpu_ms.is_none());

    let mut load = LoadAggregate::default();
    load.add(sample(
        t0,
        None,
        None,
        &[("pci:a", Some(5)), ("pci:b", Some(80))],
    ));
    load.apply(&mut metrics);
    assert_eq!(metrics.gpu_utilization_percent.unwrap().peak, 80.0);
}

#[test]
fn a_counter_that_goes_backwards_is_skipped_and_the_sample_cap_is_flagged() {
    let t0 = Instant::now();
    let mut load = LoadAggregate::default();
    load.add(sample(t0, Some((500, 1000)), None, &[]));
    load.add(sample(
        t0 + Duration::from_millis(100),
        Some((10, 1000)),
        None,
        &[],
    ));
    load.add(sample(
        t0 + Duration::from_millis(200),
        Some((600, 900)),
        None,
        &[],
    ));
    let mut metrics = PassMetrics::default();
    load.apply(&mut metrics);
    assert!(
        metrics.cpu_utilization_percent.is_none(),
        "no valid interval"
    );

    let mut load = LoadAggregate::default();
    for i in 0..(u64::from(MAX_LOAD_SAMPLES) + 5) {
        load.add(sample(
            t0 + Duration::from_millis(i * 10),
            None,
            Some((1, i)),
            &[],
        ));
    }
    load.apply(&mut metrics);
    let rss = metrics.process_rss_bytes.unwrap();
    assert_eq!(rss.coverage.samples, MAX_LOAD_SAMPLES);
    assert!(rss.coverage.capped);
}

#[test]
fn an_attached_runtime_is_never_given_process_measurements() {
    let profile = HardwareProfile {
        profile_hash: None,
        cpu: None,
        system_ram: None,
        gpus: vec![],
        issues: vec![],
        captured_at_unix_ms: 1,
    };
    let mut probe = LoadProbe::new(&profile, None);
    let sample = probe.sample();
    assert!(sample.group.is_none());
    assert!(sample.cpu.is_some());
    assert!(
        LoadProbe::new(&profile, Some(1)).pgid.is_none(),
        "never pid 1"
    );
    assert!(LoadProbe::new(&profile, Some(-3)).pgid.is_none());
}

#[test]
fn an_interval_where_idle_time_shrank_is_not_reported_as_load() {
    let t0 = Instant::now();
    let mut load = LoadAggregate::default();
    // busy, total: idle (incl. iowait) goes 100 -> 99 while busy rises by 10.
    load.add(sample(t0, Some((100, 200)), None, &[]));
    load.add(sample(
        t0 + Duration::from_millis(100),
        Some((110, 209)),
        None,
        &[],
    ));
    let mut metrics = PassMetrics::default();
    load.apply(&mut metrics);
    assert!(metrics.cpu_utilization_percent.is_none());
}

#[test]
fn a_command_name_with_invalid_utf8_still_counts_and_a_huge_proc_is_refused() {
    let dir = Dir::new();
    let mut bytes = stat_line("worker", 100, 5, 5, 10).into_bytes();
    let at = bytes.iter().position(|b| *b == b'(').unwrap() + 7;
    bytes.insert(at, 0xff);
    fs::create_dir_all(dir.0.join("100")).unwrap();
    fs::write(dir.0.join("100").join("stat"), bytes).unwrap();
    assert_eq!(read_group(&dir.0, 100, 4096).unwrap().processes, 1);
    for pid in 1000..(1000 + MAX_SCAN as u64 + 1) {
        fs::create_dir_all(dir.0.join(pid.to_string())).unwrap();
    }
    assert!(
        read_group(&dir.0, 100, 4096).is_none(),
        "partial scan is not a total"
    );
}

#[test]
fn gpu_memory_is_reported_for_the_largest_device_and_unknown_reads_are_ignored() {
    let t0 = Instant::now();
    let mut load = LoadAggregate::default();
    load.add_gpu_memory(t0, "pci:a", None);
    load.add_gpu_memory(t0, "pci:a", Some(1 << 30));
    load.add_gpu_memory(t0 + Duration::from_millis(100), "pci:a", Some(3 << 30));
    load.add_gpu_memory(t0, "pci:b", Some(2 << 30));
    let mut metrics = PassMetrics::default();
    load.apply(&mut metrics);
    let memory = metrics.gpu_memory_bytes.unwrap();
    assert_eq!(memory.peak, (3u64 << 30) as f64);
    assert_eq!(memory.coverage.samples, 2);
    assert_eq!(memory.coverage.scope, Scope::Device);
    assert!(PassMetrics::default().gpu_memory_bytes.is_none());
}
