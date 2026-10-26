// SPDX-License-Identifier: GPL-3.0-or-later
//! Finds files already on disk whose content matches a catalog artifact's SHA-256.
//!
//! Read-only: nothing is copied, linked, moved or removed. Only regular files inside
//! the known model directories are considered; symlinks and special files are skipped,
//! each candidate is opened without following links and re-checked after hashing, and a
//! file name that claims a digest is never trusted. Call from a blocking worker.

use crate::{
    db::{HistoryStore, digests::FileStamp},
    models::catalog::canonical_sha256,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    Ollama,
    HuggingFace,
    Application,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DedupRoot {
    pub kind: RootKind,
    pub path: PathBuf,
}

/// An artifact to look for: it must have a published size and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub artifact_id: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMatch {
    pub artifact_id: String,
    pub path: PathBuf,
    pub root: RootKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootState {
    Scanned,
    /// The directory does not exist (or is not a directory): nothing was looked at.
    Missing,
    /// The directory exists but could not be read: nothing was looked at.
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootStatus {
    pub kind: RootKind,
    pub path: PathBuf,
    pub state: RootState,
}

/// Why a scan ended before every file was considered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    EntryLimit,
    ByteLimit,
    TimeLimit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub matches: Vec<LocalMatch>,
    pub roots: Vec<RootStatus>,
    /// `Some` means the result is partial: absence of a match proves nothing.
    pub stopped: Option<Stop>,
    pub entries: u64,
    pub hashed_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub entries: u64,
    pub hashed_bytes: u64,
    pub time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 100_000,
            hashed_bytes: 256 * 1024 * 1024 * 1024,
            time: Duration::from_secs(600),
        }
    }
}

const READ_CHUNK: usize = 1024 * 1024;

/// The directories where models usually live, from the environment. Nonexistent ones
/// are reported by [`scan`] as not scanned.
pub fn default_roots() -> Vec<DedupRoot> {
    roots_from(|name| std::env::var_os(name))
}

fn roots_from(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<DedupRoot> {
    let absolute = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let home = absolute("HOME");
    let mut roots = Vec::new();
    let mut add = |kind, path: PathBuf| {
        if !roots.iter().any(|root: &DedupRoot| root.path == path) {
            roots.push(DedupRoot { kind, path });
        }
    };
    if let Some(models) = absolute("OLLAMA_MODELS") {
        add(RootKind::Ollama, models.join("blobs"));
    }
    if let Some(home) = &home {
        add(RootKind::Ollama, home.join(".ollama/models/blobs"));
    }
    add(
        RootKind::Ollama,
        PathBuf::from("/usr/share/ollama/.ollama/models/blobs"),
    );
    let hub = absolute("HF_HUB_CACHE")
        .or_else(|| absolute("HF_HOME").map(|dir| dir.join("hub")))
        .or_else(|| absolute("XDG_CACHE_HOME").map(|dir| dir.join("huggingface/hub")))
        .or_else(|| home.as_ref().map(|dir| dir.join(".cache/huggingface/hub")));
    if let Some(hub) = hub {
        add(RootKind::HuggingFace, hub);
    }
    let data = absolute("XDG_DATA_HOME").or_else(|| home.as_ref().map(|d| d.join(".local/share")));
    if let Some(data) = data {
        add(
            RootKind::Application,
            data.join("local-ai-workstation/models/blobs"),
        );
    }
    roots
}

/// Look for each wanted artifact's content in `roots`.
///
/// Only files whose size equals a wanted size are hashed. `digests`, when given, lets a
/// file whose device, inode, size and both timestamps are unchanged skip hashing.
pub fn scan(
    roots: &[DedupRoot],
    wanted: &[Wanted],
    digests: Option<&HistoryStore>,
    cancel: &AtomicBool,
    limits: &Limits,
) -> ScanReport {
    let mut by_size: HashMap<u64, Vec<&Wanted>> = HashMap::new();
    for item in wanted {
        if item.size > 0 && canonical_sha256(&item.sha256).as_deref() == Some(&item.sha256) {
            by_size.entry(item.size).or_default().push(item);
        }
    }
    let mut run = Run {
        by_size,
        digests,
        cancel,
        limits,
        started: Instant::now(),
        report: ScanReport::default(),
    };
    for root in roots {
        if run.report.stopped.is_some() {
            break;
        }
        run.scan_root(root);
    }
    run.report
}

struct Run<'a> {
    by_size: HashMap<u64, Vec<&'a Wanted>>,
    digests: Option<&'a HistoryStore>,
    cancel: &'a AtomicBool,
    limits: &'a Limits,
    started: Instant,
    report: ScanReport,
}

impl Run<'_> {
    fn should_stop(&mut self) -> bool {
        if self.report.stopped.is_none() {
            self.report.stopped = if self.cancel.load(Ordering::Relaxed) {
                Some(Stop::Cancelled)
            } else if self.started.elapsed() > self.limits.time {
                Some(Stop::TimeLimit)
            } else {
                None
            };
        }
        self.report.stopped.is_some()
    }

    fn scan_root(&mut self, root: &DedupRoot) {
        let status = |state| RootStatus {
            kind: root.kind,
            path: root.path.clone(),
            state,
        };
        // The root itself may be a link (for example ~/.ollama); nothing inside may be.
        let base = match fs::canonicalize(&root.path) {
            Ok(base) if base.is_dir() => base,
            Ok(_) => return self.report.roots.push(status(RootState::Missing)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.report.roots.push(status(RootState::Missing));
            }
            Err(_) => return self.report.roots.push(status(RootState::Unreadable)),
        };
        let Some(directories) = blob_directories(root.kind, &base) else {
            return self.report.roots.push(status(RootState::Unreadable));
        };
        self.report.roots.push(status(RootState::Scanned));
        for directory in directories {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                if self.should_stop() {
                    return;
                }
                self.report.entries += 1;
                if self.report.entries > self.limits.entries {
                    self.report.stopped = Some(Stop::EntryLimit);
                    return;
                }
                self.consider(root.kind, &base, &entry.path());
            }
        }
    }

    /// One directory entry: skip anything that is not a plain file of a wanted size.
    fn consider(&mut self, kind: RootKind, base: &Path, path: &Path) {
        let Ok(listed) = fs::symlink_metadata(path) else {
            return;
        };
        if !listed.file_type().is_file() || !self.by_size.contains_key(&listed.len()) {
            return;
        }
        // O_NOFOLLOW refuses a link swapped in after the check; O_NONBLOCK keeps a
        // pipe swapped in from hanging the open.
        let Ok(file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        else {
            return;
        };
        let Ok(opened) = file.metadata() else {
            return;
        };
        if !opened.is_file() || !same_file(&listed, &opened) {
            return;
        }
        if !fs::canonicalize(path).is_ok_and(|resolved| resolved.starts_with(base)) {
            return;
        }
        let stamp = stamp_of(&opened);
        let key = path.to_str();
        let cached = key.and_then(|key| self.digests?.cached_file_digest(key, &stamp));
        let digest = match cached {
            Some(digest) => digest,
            None => {
                let size = opened.len();
                if self.report.hashed_bytes.saturating_add(size) > self.limits.hashed_bytes {
                    self.report.stopped = Some(Stop::ByteLimit);
                    return;
                }
                let Some(digest) = self.hash(file, size) else {
                    return;
                };
                // The file must still be the same one, in the same state, at the same path.
                let unchanged = fs::symlink_metadata(path)
                    .is_ok_and(|now| now.file_type().is_file() && same_file(&now, &opened));
                let after = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(path)
                    .and_then(|file| file.metadata());
                if !unchanged || !after.is_ok_and(|after| stamp_of(&after) == stamp) {
                    return;
                }
                if let (Some(store), Some(key)) = (self.digests, key) {
                    let _ = store.cache_file_digest(key, &stamp, &digest, now_ms());
                }
                digest
            }
        };
        for item in &self.by_size[&opened.len()] {
            if item.sha256 == digest {
                self.report.matches.push(LocalMatch {
                    artifact_id: item.artifact_id.clone(),
                    path: path.to_path_buf(),
                    root: kind,
                });
            }
        }
    }

    /// Streams the file; `None` if it was cancelled, timed out, shrank or grew.
    fn hash(&mut self, mut file: File, size: u64) -> Option<String> {
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; READ_CHUNK];
        let mut total = 0u64;
        loop {
            if self.should_stop() {
                return None;
            }
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > size {
                return None;
            }
            self.report.hashed_bytes += read as u64;
            hasher.update(&buffer[..read]);
        }
        (total == size).then(|| {
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
    }
}

/// Directories that directly hold blob files for one root.
fn blob_directories(kind: RootKind, base: &Path) -> Option<Vec<PathBuf>> {
    if kind != RootKind::HuggingFace {
        return Some(vec![base.to_path_buf()]);
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(base).ok()?.flatten() {
        let is_model = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("models--"));
        let blobs = entry.path().join("blobs");
        let real_dirs = entry.file_type().is_ok_and(|t| t.is_dir())
            && fs::symlink_metadata(&blobs).is_ok_and(|m| m.file_type().is_dir());
        if is_model && real_dirs {
            directories.push(blobs);
        }
    }
    Some(directories)
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    (
        a.dev(),
        a.ino(),
        a.len(),
        a.mtime(),
        a.mtime_nsec(),
        a.ctime(),
        a.ctime_nsec(),
    ) == (
        b.dev(),
        b.ino(),
        b.len(),
        b.mtime(),
        b.mtime_nsec(),
        b.ctime(),
        b.ctime_nsec(),
    )
}

fn stamp_of(metadata: &Metadata) -> FileStamp {
    let nanos = |secs: i64, sub: i64| i128::from(secs) * 1_000_000_000 + i128::from(sub);
    FileStamp {
        dev: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mtime_ns: nanos(metadata.mtime(), metadata.mtime_nsec()),
        ctime_ns: nanos(metadata.ctime(), metadata.ctime_nsec()),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<std::ffi::OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).into())
        }
    }

    fn paths(roots: &[DedupRoot]) -> Vec<(RootKind, &str)> {
        roots
            .iter()
            .map(|r| (r.kind, r.path.to_str().unwrap()))
            .collect()
    }

    #[test]
    fn roots_follow_the_documented_environment_precedence() {
        let roots = roots_from(env(&[
            ("HOME", "/home/u"),
            ("OLLAMA_MODELS", "/data/ollama"),
            ("HF_HOME", "/data/hf"),
            ("XDG_DATA_HOME", "/data/share"),
        ]));
        assert_eq!(
            paths(&roots),
            [
                (RootKind::Ollama, "/data/ollama/blobs"),
                (RootKind::Ollama, "/home/u/.ollama/models/blobs"),
                (RootKind::Ollama, "/usr/share/ollama/.ollama/models/blobs"),
                (RootKind::HuggingFace, "/data/hf/hub"),
                (
                    RootKind::Application,
                    "/data/share/local-ai-workstation/models/blobs"
                ),
            ]
        );
        let roots = roots_from(env(&[
            ("HOME", "/home/u"),
            ("HF_HUB_CACHE", "/hub"),
            ("HF_HOME", "/ignored"),
        ]));
        assert!(paths(&roots).contains(&(RootKind::HuggingFace, "/hub")));
        let roots = roots_from(env(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/cache")]));
        assert!(paths(&roots).contains(&(RootKind::HuggingFace, "/cache/huggingface/hub")));
        let roots = roots_from(env(&[("HOME", "/home/u")]));
        assert!(paths(&roots).contains(&(RootKind::HuggingFace, "/home/u/.cache/huggingface/hub")));
        assert!(paths(&roots).contains(&(
            RootKind::Application,
            "/home/u/.local/share/local-ai-workstation/models/blobs"
        )));
        // Relative values are ignored rather than resolved against the working directory.
        let roots = roots_from(env(&[("OLLAMA_MODELS", "relative"), ("HOME", "rel")]));
        assert_eq!(
            paths(&roots),
            [(RootKind::Ollama, "/usr/share/ollama/.ollama/models/blobs")]
        );
    }
}
