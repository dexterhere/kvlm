// SPDX-License-Identifier: GPL-3.0-or-later
//! The private record of the one process this application started, so a crash of the
//! application (which `kill_on_drop` cannot cover) does not leave an orphan forever.
//!
//! A recorded process is signalled only when every fact matches: the pid is alive, its
//! start time is the recorded one, its executable is the recorded path inside the install
//! root, and it leads its own process group. Anything less is left alone.

use rustix::process::{Pid, Signal, getpgid, kill_process_group, test_kill_process};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_RECORD: u64 = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub pid: u32,
    /// Field 22 of `/proc/<pid>/stat`: clock ticks since boot at process start.
    pub start_ticks: u64,
    pub exe: PathBuf,
    pub generation: String,
}

/// Start time of `pid`, or `None` when it does not exist.
pub fn start_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces; fields follow the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

pub fn write(path: &Path, record: &Record) -> std::io::Result<()> {
    let json = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&json)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

pub fn read(path: &Path) -> Option<Record> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .ok()?;
    let mut text = String::new();
    file.take(MAX_RECORD).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reclaim {
    /// No record, or the recorded process was already gone.
    Nothing,
    /// The orphan was proven ours and has been terminated.
    Reclaimed,
    /// Something is there that this application cannot prove it started; left alone.
    Skipped(&'static str),
}

fn pid_of(raw: u32) -> Option<Pid> {
    i32::try_from(raw)
        .ok()
        .filter(|p| *p > 1)
        .and_then(Pid::from_raw)
}

/// Terminate a recorded orphan from an earlier run of the application, if provably ours.
pub async fn reclaim(path: &Path, install_root: &Path, grace: Duration) -> Reclaim {
    let Some(record) = read(path) else {
        remove(path);
        return Reclaim::Nothing;
    };
    let Some(pid) = pid_of(record.pid) else {
        remove(path);
        return Reclaim::Nothing;
    };
    if test_kill_process(pid).is_err() && start_ticks(record.pid).is_none() {
        remove(path);
        return Reclaim::Nothing;
    }
    let Some(ticks) = start_ticks(record.pid) else {
        remove(path);
        return Reclaim::Nothing;
    };
    if ticks != record.start_ticks {
        remove(path);
        return Reclaim::Skipped("the process id was reused");
    }
    let exe = fs::read_link(format!("/proc/{}/exe", record.pid)).ok();
    if exe.as_deref() != Some(record.exe.as_path()) || !record.exe.starts_with(install_root) {
        return Reclaim::Skipped("the process is not the recorded runtime");
    }
    if getpgid(Some(pid)).ok() != Some(pid) {
        return Reclaim::Skipped("the process does not lead its own group");
    }
    let alive = || start_ticks(record.pid) == Some(ticks);
    let _ = kill_process_group(pid, Signal::TERM);
    let deadline = tokio::time::Instant::now() + grace;
    while alive() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if alive() {
        let _ = kill_process_group(pid, Signal::KILL);
        for _ in 0..40 {
            if !alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    remove(path);
    Reclaim::Reclaimed
}
