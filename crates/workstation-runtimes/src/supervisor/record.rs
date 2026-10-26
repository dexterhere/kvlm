// SPDX-License-Identifier: GPL-3.0-or-later
//! The private record of the one process this application started, so a crash of the
//! application (which `kill_on_drop` cannot cover) does not leave an orphan forever.
//!
//! A recorded process is signalled only when every fact matches: the pid is alive, its
//! start time is the recorded one, its executable is the recorded path inside the install
//! root, and it leads its own process group. Anything less is left alone.

use rustix::process::{Pid, PidfdFlags, Signal, getpgid, pidfd_open, pidfd_send_signal};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
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

/// State letter and start time of `pid` from `/proc/<pid>/stat`, or `None` when it does not exist.
fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces; fields follow the last ')'.
    let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    Some((state, fields.nth(18)?.parse().ok()?))
}

/// Start time of `pid` (clock ticks since boot), or `None` when it does not exist.
pub fn start_ticks(pid: u32) -> Option<u64> {
    proc_stat(pid).map(|(_, ticks)| ticks)
}

/// Exclusive hold on the record for the life of one supervisor, so two application
/// instances can neither overwrite each other's record nor reclaim each other's runtime.
pub struct ProcessLock {
    _file: File,
}

impl ProcessLock {
    pub fn acquire(record_path: &Path) -> Option<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(record_path.with_extension("lock"))
            .ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        // A concurrent fork in this process shares the open file description until its exec
        // closes it, so a few short retries ride that out; a real holder still reads as busy.
        for attempt in 0..5 {
            if rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .is_ok()
            {
                return Some(Self { _file: file });
            }
            if attempt < 4 {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        None
    }
}

pub fn write(path: &Path, record: &Record) -> std::io::Result<()> {
    let json = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp); // a leftover from a crash; the process lock makes us the only writer
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
    // NONBLOCK so a FIFO planted at this name cannot hang the open; only a regular file is read.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
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
/// Needs the process lock, so it cannot race another instance's supervisor.
///
/// The process is held by a pidfd from before the identity checks to the last signal, so a
/// pid reused in between can never be signalled. Only the recorded leader is signalled; the
/// runtime is a single process.
pub async fn reclaim(
    _lock: &ProcessLock,
    path: &Path,
    install_root: &Path,
    grace: Duration,
) -> Reclaim {
    let Some(record) = read(path) else {
        remove(path);
        return Reclaim::Nothing;
    };
    let Some(pid) = pid_of(record.pid) else {
        remove(path);
        return Reclaim::Nothing;
    };
    let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => {
            remove(path); // only ESRCH proves the process is gone
            return Reclaim::Nothing;
        }
        // Out of descriptors or memory says nothing about the process: keep the record.
        Err(_) => return Reclaim::Skipped("the process could not be pinned"),
    };
    // Checked after the pidfd exists: whatever these read is the process the pidfd names.
    let Some((state, ticks)) = proc_stat(record.pid) else {
        remove(path);
        return Reclaim::Nothing;
    };
    if state == 'Z' {
        remove(path);
        return Reclaim::Nothing;
    }
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
    let gone = || proc_stat(record.pid).is_none_or(|(s, t)| s == 'Z' || t != ticks);
    let _ = pidfd_send_signal(&pidfd, Signal::TERM);
    let deadline = tokio::time::Instant::now() + grace;
    while !gone() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !gone() {
        let _ = pidfd_send_signal(&pidfd, Signal::KILL);
        for _ in 0..40 {
            if gone() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    if gone() {
        remove(path);
        Reclaim::Reclaimed
    } else {
        Reclaim::Skipped("the process did not exit")
    }
}
