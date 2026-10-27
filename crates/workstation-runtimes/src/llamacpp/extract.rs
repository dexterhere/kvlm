// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming `.tar.gz` extraction into a private directory the caller just created.
//!
//! Nothing in the archive chooses where bytes go: every path component is validated, the
//! single top-level directory is stripped, files are created with `O_EXCL|O_NOFOLLOW`, and
//! symlinks are created only after all files exist and only when they point at a sibling
//! file name that is itself part of the archive. Sizes are counted on the decompressed
//! stream, so a compression bomb stops at the cap rather than after it.

use flate2::read::GzDecoder;
use std::{
    collections::{HashMap, HashSet},
    fs::{DirBuilder, OpenOptions},
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, symlink},
    path::{Component, Path, PathBuf},
};
use tar::{Archive, EntryType};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_entries: usize,
    pub max_bytes: u64,
}

pub const LIMITS: Limits = Limits {
    max_entries: 2_000,
    max_bytes: 5 << 29, // 2.5 GiB
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The archive tried something it may not (path, type, link, duplicate, layout).
    Unsafe(&'static str),
    TooManyEntries,
    TooLarge,
    Cancelled,
    NoSpace,
    /// Not a readable archive.
    Corrupt,
    Io,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsafe(why) => return write!(f, "The archive was refused: {why}."),
            Self::TooManyEntries => "The archive has more entries than allowed.",
            Self::TooLarge => "The archive expands beyond the size limit.",
            Self::Cancelled => "The install was cancelled.",
            Self::NoSpace => "Not enough free disk space to extract the archive.",
            Self::Corrupt => "The archive is damaged.",
            Self::Io => "Cannot write the extracted files. Check free disk space.",
        })
    }
}
impl std::error::Error for ExtractError {}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub entries: usize,
    pub bytes: u64,
}

/// Longest GNU long-name/long-link body accepted. Real names are far shorter.
const MAX_LONG_NAME: u64 = 4096;
/// Free space that must remain while extracting, checked every `CHECK_EVERY` bytes.
const MIN_FREE: u64 = 512 << 20;
const CHECK_EVERY: u64 = 32 << 20;

/// Counts decompressed bytes, stops at the cap, notices cancellation, keeps a disk reserve,
/// and inspects tar header blocks as they pass so that metadata the `tar` crate would buffer
/// whole (pax, sparse, oversized long names) is refused before it is read.
struct Guarded<'a, R> {
    inner: R,
    left: u64,
    cancel: &'a (dyn Fn() -> bool + Sync),
    dest: &'a Path,
    since_check: u64,
    header: [u8; 512],
    filled: usize,
    skip: u64,
    failure: Option<ExtractError>,
}

impl<R: Read> Guarded<'_, R> {
    fn fail(&mut self, error: ExtractError) -> io::Error {
        self.failure = Some(error);
        io::Error::new(io::ErrorKind::InvalidData, "refused")
    }

    /// Walk the bytes just read, tracking where header blocks start.
    fn scan(&mut self, mut data: &[u8]) -> Result<(), ExtractError> {
        while !data.is_empty() {
            if self.skip > 0 {
                let step = self.skip.min(data.len() as u64);
                self.skip -= step;
                data = &data[step as usize..];
                continue;
            }
            let take = (512 - self.filled).min(data.len());
            self.header[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled < 512 {
                continue;
            }
            self.filled = 0;
            // A size this scanner cannot read (the base-256 form, garbage) would let the tar
            // reader and the scanner disagree about where the next header starts.
            let size = octal(&self.header[124..136])
                .ok_or(ExtractError::Unsafe("unsupported tar size field"))?;
            match self.header[156] {
                b'x' | b'g' | b'S' => return Err(ExtractError::Unsafe("extended tar headers")),
                b'L' | b'K' if size > MAX_LONG_NAME => {
                    return Err(ExtractError::Unsafe("oversized long name"));
                }
                _ => {}
            }
            self.skip = size.div_ceil(512) * 512;
        }
        Ok(())
    }
}

/// Plain octal size field; the base-256 form used for huge sizes is not accepted.
fn octal(field: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(field).ok()?;
    let digits = text.trim_matches(|c: char| c == '\0' || c == ' ');
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, 8).ok()
}

impl<R: Read> Read for Guarded<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if (self.cancel)() {
            return Err(self.fail(ExtractError::Cancelled));
        }
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            return Err(self.fail(ExtractError::TooLarge));
        }
        self.left -= n as u64;
        self.since_check += n as u64;
        if self.since_check >= CHECK_EVERY {
            self.since_check = 0;
            if free_bytes(self.dest).is_none_or(|free| free < MIN_FREE) {
                return Err(self.fail(ExtractError::NoSpace));
            }
        }
        if let Err(error) = self.scan(&buf[..n]) {
            return Err(self.fail(error));
        }
        Ok(n)
    }
}

fn free_bytes(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

fn valid_component(part: &str) -> bool {
    (1..=128).contains(&part.len())
        && part != "."
        && part != ".."
        && !part.starts_with('-')
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

/// Relative components below the top-level directory, or `None` for the directory itself.
fn relative(path: &Path, top: &mut Option<String>) -> Result<Option<Vec<String>>, ExtractError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .filter(|n| valid_component(n))
                    .ok_or(ExtractError::Unsafe("unsupported file name"))?;
                parts.push(name.to_owned());
            }
            _ => return Err(ExtractError::Unsafe("absolute or parent path")),
        }
    }
    let Some(first) = parts.first() else {
        return Err(ExtractError::Unsafe("empty path"));
    };
    match top {
        Some(known) if known != first => {
            return Err(ExtractError::Unsafe("more than one top-level directory"));
        }
        Some(_) => {}
        None => *top = Some(first.clone()),
    }
    parts.remove(0);
    if parts.len() > 4 {
        return Err(ExtractError::Unsafe("path nested too deeply"));
    }
    Ok((!parts.is_empty()).then_some(parts))
}

fn io_error(_: io::Error) -> ExtractError {
    ExtractError::Io
}

/// Extract `archive` into the existing directory `dest`. May be called for several archives
/// with the same `dest`; a file present in two of them is refused. The returned summary is
/// what this archive used, so the caller can shrink the limits for the next one.
pub fn extract_tar_gz(
    archive: impl Read,
    dest: &Path,
    limits: Limits,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Summary, ExtractError> {
    if free_bytes(dest).is_none_or(|free| free < MIN_FREE) {
        return Err(ExtractError::NoSpace);
    }
    let mut stream = Guarded {
        inner: GzDecoder::new(archive),
        left: limits.max_bytes,
        cancel,
        dest,
        since_check: 0,
        header: [0; 512],
        filled: 0,
        skip: 0,
        failure: None,
    };
    let result = unpack(&mut stream, dest, limits);
    match result {
        // The guard's reason is more useful than the tar crate's "damaged".
        Err(_) if stream.failure.is_some() => Err(stream.failure.take().unwrap()),
        Ok(mut summary) => {
            summary.bytes = limits.max_bytes - stream.left;
            Ok(summary)
        }
        other => other,
    }
}

fn unpack(
    stream: &mut Guarded<'_, GzDecoder<impl Read>>,
    dest: &Path,
    limits: Limits,
) -> Result<Summary, ExtractError> {
    let mut archive = Archive::new(&mut *stream);
    let mut top: Option<String> = None;
    let mut files: HashSet<PathBuf> = HashSet::new();
    let mut links: HashMap<PathBuf, String> = HashMap::new();
    let mut summary = Summary::default();
    for entry in archive.entries().map_err(|_| ExtractError::Corrupt)? {
        let mut entry = entry.map_err(|_| ExtractError::Corrupt)?;
        summary.entries += 1;
        if summary.entries > limits.max_entries {
            return Err(ExtractError::TooManyEntries);
        }
        let kind = entry.header().entry_type();
        if matches!(kind, EntryType::XGlobalHeader | EntryType::XHeader) {
            continue;
        }
        let path = entry
            .path()
            .map_err(|_| ExtractError::Corrupt)?
            .into_owned();
        let Some(parts) = relative(&path, &mut top)? else {
            if kind.is_dir() {
                continue;
            }
            return Err(ExtractError::Unsafe("file outside a top-level directory"));
        };
        let target: PathBuf = parts.iter().collect();
        let full = dest.join(&target);
        let parent = full.parent().ok_or(ExtractError::Unsafe("bad path"))?;
        if kind.is_dir() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&full)
                .map_err(io_error)?;
        } else if kind.is_file() {
            if entry.size() > limits.max_bytes {
                return Err(ExtractError::TooLarge);
            }
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(io_error)?;
            let executable = entry.header().mode().is_ok_and(|m| m & 0o111 != 0);
            if !files.insert(target.clone()) || links.contains_key(&target) {
                return Err(ExtractError::Unsafe("duplicate file name"));
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc_nofollow())
                .mode(if executable { 0o700 } else { 0o600 })
                .open(&full)
                .map_err(|e| match e.kind() {
                    io::ErrorKind::AlreadyExists => ExtractError::Unsafe("file already exists"),
                    _ => ExtractError::Io,
                })?;
            let written = io::copy(&mut entry, &mut file).map_err(|_| ExtractError::Corrupt)?;
            let _ = written;
        } else if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|_| ExtractError::Corrupt)?
                .ok_or(ExtractError::Unsafe("link without a target"))?;
            let name = link
                .to_str()
                .filter(|n| valid_component(n))
                .ok_or(ExtractError::Unsafe(
                    "link target is not a sibling file name",
                ))?
                .to_owned();
            if files.contains(&target) || links.insert(target, name).is_some() {
                return Err(ExtractError::Unsafe("duplicate file name"));
            }
        } else {
            return Err(ExtractError::Unsafe("unsupported entry type"));
        }
    }
    if top.is_none() {
        return Err(ExtractError::Corrupt);
    }
    // Links last: each must point at a sibling that exists as a file or as another link.
    for (target, name) in &links {
        let sibling = target.with_file_name(name);
        if sibling == *target || !(files.contains(&sibling) || links.contains_key(&sibling)) {
            return Err(ExtractError::Unsafe("link target is missing"));
        }
        // Chains are followed only a few hops, so a cycle cannot pass.
        let mut here = sibling;
        for _ in 0..4 {
            match links.get(&here) {
                Some(next) => here = here.with_file_name(next),
                None => break,
            }
        }
        if !files.contains(&here) {
            return Err(ExtractError::Unsafe("link does not resolve to a file"));
        }
        symlink(name, dest.join(target)).map_err(io_error)?;
    }
    Ok(summary)
}

fn libc_nofollow() -> i32 {
    rustix::fs::OFlags::NOFOLLOW.bits() as i32
}
