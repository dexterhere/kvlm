// SPDX-License-Identifier: GPL-3.0-or-later
//! The application's own model store: private directories, a cross-instance lock, partial
//! files addressed only by SHA-256, and verified publication into `blobs/`.
//!
//! Layout under the models directory (mode 0700, owned by the current user):
//! `.lock`, `partial/<sha256>.part`, `blobs/sha256-<sha256>`. File names derive only from
//! a validated digest, never from anything a server supplied. Every open refuses links,
//! and every file must be a regular, single-link file owned by the current user, so a
//! planted symlink or hard link can never cause an external file to be written.
//! Call from a blocking worker.

use crate::models::catalog::canonical_sha256;
use rustix::fs::{RenameFlags, renameat_with};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, DirBuilder, File, Metadata, OpenOptions, TryLockError},
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Free space that must remain after the bytes still needed are written.
pub const RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Most bytes all retained partial files may add up to.
pub const PARTIAL_BUDGET_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    /// Another instance holds the lock.
    InUse,
    /// A directory or file is not private to this user, or is a link.
    Unsafe(&'static str),
    BadDigest,
    /// The file on disk is longer than the artifact's published size.
    PartTooLong,
    DiskFull,
    NotEnoughSpace {
        needed: u64,
        available: u64,
    },
    PartialBudget,
    /// Content did not match the expected SHA-256.
    Integrity,
    Io(io::ErrorKind),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InUse => {
                f.write_str("Another instance of the application is using the model store.")
            }
            Self::Unsafe(why) => write!(f, "The model store is not safe to use: {why}."),
            Self::BadDigest => f.write_str("The checksum is not a valid SHA-256."),
            Self::PartTooLong => {
                f.write_str("The partial file is longer than the model; discard it and retry.")
            }
            Self::DiskFull => f.write_str("The disk is full. Free space and resume."),
            Self::NotEnoughSpace { .. } => f.write_str(
                "Not enough free disk space. Keep 2 GiB free beyond the model size, then resume.",
            ),
            Self::PartialBudget => f.write_str(
                "Too much partial data is kept (64 GiB). Discard an unfinished download first.",
            ),
            Self::Integrity => {
                f.write_str("The downloaded file did not match its checksum and was discarded.")
            }
            Self::Io(kind) => write!(f, "The model store could not be used ({kind})."),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::StorageFull {
            Self::DiskFull
        } else {
            Self::Io(error.kind())
        }
    }
}

/// What publishing a verified download did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    /// The verified file is now at its final path.
    New,
    /// A verified file with this digest was already there; the part file was removed.
    Reused,
}

/// An open, exclusively locked model store. Dropping it releases the lock.
pub struct Storage {
    root: PathBuf,
    _lock: File,
}

fn euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn ensure_private_dir(path: &Path) -> Result<(), StorageError> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_dir() {
        return Err(StorageError::Unsafe(
            "a store directory is a link or not a directory",
        ));
    }
    if meta.uid() != euid() {
        return Err(StorageError::Unsafe(
            "a store directory is owned by another user",
        ));
    }
    if meta.permissions().mode() & 0o777 != 0o700 {
        return Err(StorageError::Unsafe(
            "a store directory is not private (mode 0700)",
        ));
    }
    Ok(())
}

/// Regular, single link, owned by this user.
fn check_file(meta: &Metadata) -> Result<(), StorageError> {
    if !meta.is_file() {
        return Err(StorageError::Unsafe("a store file is not a regular file"));
    }
    if meta.nlink() != 1 {
        return Err(StorageError::Unsafe(
            "a store file has more than one hard link",
        ));
    }
    if meta.uid() != euid() {
        return Err(StorageError::Unsafe(
            "a store file is owned by another user",
        ));
    }
    Ok(())
}

fn open_private(path: &Path, create: bool) -> Result<File, StorageError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        // No links; and a pipe swapped in must not hang the open.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_file(&file.metadata()?)?;
    Ok(file)
}

impl Storage {
    /// Create (if needed), verify and lock the store at `models_dir`.
    pub fn open(models_dir: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = models_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        ensure_private_dir(models_dir)?;
        ensure_private_dir(&models_dir.join("partial"))?;
        ensure_private_dir(&models_dir.join("blobs"))?;
        let lock = open_private(&models_dir.join(".lock"), true)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(StorageError::InUse),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        Ok(Self {
            root: models_dir.to_path_buf(),
            _lock: lock,
        })
    }

    fn part_path(&self, sha256: &str) -> Result<PathBuf, StorageError> {
        digest(sha256)?;
        Ok(self.root.join("partial").join(format!("{sha256}.part")))
    }

    fn final_path(&self, sha256: &str) -> Result<PathBuf, StorageError> {
        digest(sha256)?;
        Ok(self.root.join("blobs").join(format!("sha256-{sha256}")))
    }

    /// Where a verified file lives (it may not exist).
    pub fn blob_path(&self, sha256: &str) -> Result<PathBuf, StorageError> {
        self.final_path(sha256)
    }

    /// The partial file for a digest, created empty if needed, positioned at its end.
    /// Fails if it is longer than `expected_size`.
    pub fn open_part(&self, sha256: &str, expected_size: u64) -> Result<(File, u64), StorageError> {
        let mut file = open_private(&self.part_path(sha256)?, true)?;
        let length = file.metadata()?.len();
        if length > expected_size {
            return Err(StorageError::PartTooLong);
        }
        file.seek(SeekFrom::End(0))?;
        Ok((file, length))
    }

    /// Size of the partial file, if there is one.
    pub fn part_len(&self, sha256: &str) -> Result<Option<u64>, StorageError> {
        match open_private_existing(&self.part_path(sha256)?)? {
            Some(file) => Ok(Some(file.metadata()?.len())),
            None => Ok(None),
        }
    }

    pub fn discard_part(&self, sha256: &str) -> Result<(), StorageError> {
        match fs::remove_file(self.part_path(sha256)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Empty a partial file that must restart from zero.
    pub fn restart_part(&self, file: &mut File) -> Result<(), StorageError> {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        Ok(())
    }

    /// Hash the first `length` bytes of an open partial file, leaving it positioned at
    /// the end, so the download can continue the same digest.
    pub fn hash_prefix(&self, file: &mut File, length: u64) -> Result<Sha256, StorageError> {
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut remaining = length;
        while remaining > 0 {
            let want = buffer
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            let read = file.read(&mut buffer[..want])?;
            if read == 0 {
                return Err(StorageError::Io(io::ErrorKind::UnexpectedEof));
            }
            hasher.update(&buffer[..read]);
            remaining -= read as u64;
        }
        file.seek(SeekFrom::End(0))?;
        Ok(hasher)
    }

    /// Bytes available to unprivileged writes on the store's filesystem.
    pub fn free_bytes(&self) -> Result<u64, StorageError> {
        let stat = rustix::fs::statvfs(&self.root).map_err(io::Error::from)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    /// Refuse to write `still_needed` more bytes unless a 2 GiB reserve would remain.
    pub fn check_space(&self, still_needed: u64) -> Result<(), StorageError> {
        let available = self.free_bytes()?;
        let needed = still_needed.saturating_add(RESERVE_BYTES);
        if available < needed {
            return Err(StorageError::NotEnoughSpace { needed, available });
        }
        Ok(())
    }

    /// Bytes held in all partial files.
    pub fn partial_total(&self) -> Result<u64, StorageError> {
        let mut total = 0u64;
        for entry in fs::read_dir(self.root.join("partial"))?.flatten() {
            if let Ok(meta) = fs::symlink_metadata(entry.path())
                && meta.file_type().is_file()
            {
                total = total.saturating_add(meta.len());
            }
        }
        Ok(total)
    }

    /// Admit a new partial file only if the retained partials stay within the budget.
    pub fn check_partial_budget(
        &self,
        sha256: &str,
        expected_size: u64,
    ) -> Result<(), StorageError> {
        let held = self.part_len(sha256)?.unwrap_or(0);
        let others = self.partial_total()?.saturating_sub(held);
        if others.saturating_add(expected_size) > PARTIAL_BUDGET_BYTES {
            return Err(StorageError::PartialBudget);
        }
        Ok(())
    }

    /// Whether a verified file with this digest and size is already published. The file is
    /// re-hashed; a wrong one is deleted.
    pub fn verified_blob(&self, sha256: &str, size: u64) -> Result<bool, StorageError> {
        let path = self.final_path(sha256)?;
        let Some(mut file) = open_private_existing(&path)? else {
            return Ok(false);
        };
        let good = file.metadata()?.len() == size && {
            let hasher = self.hash_prefix(&mut file, size)?;
            hex(hasher) == sha256
        };
        if !good {
            let _ = fs::remove_file(&path);
        }
        Ok(good)
    }

    /// Check the finished digest, fsync, and publish without ever overwriting a good file.
    /// A mismatch deletes the partial file.
    pub fn publish(
        &self,
        sha256: &str,
        part: File,
        hasher: Sha256,
    ) -> Result<Published, StorageError> {
        if hex(hasher) != sha256 {
            self.discard_part(sha256)?;
            return Err(StorageError::Integrity);
        }
        part.sync_all()?;
        let (from, to) = (self.part_path(sha256)?, self.final_path(sha256)?);
        let outcome = match renameat_with(
            rustix::fs::CWD,
            &from,
            rustix::fs::CWD,
            &to,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Published::New,
            Err(rustix::io::Errno::EXIST) => {
                // Something is already published under this digest: keep it only if it is good.
                let size = part.metadata()?.len();
                if self.verified_blob(sha256, size)? {
                    self.discard_part(sha256)?;
                    Published::Reused
                } else {
                    renameat_with(
                        rustix::fs::CWD,
                        &from,
                        rustix::fs::CWD,
                        &to,
                        RenameFlags::NOREPLACE,
                    )
                    .map_err(io::Error::from)?;
                    Published::New
                }
            }
            Err(error) => return Err(io::Error::from(error).into()),
        };
        File::open(self.root.join("blobs"))?.sync_all()?;
        Ok(outcome)
    }
}

fn open_private_existing(path: &Path) -> Result<Option<File>, StorageError> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => {
            check_file(&file.metadata()?)?;
            Ok(Some(file))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn digest(sha256: &str) -> Result<(), StorageError> {
    (canonical_sha256(sha256).as_deref() == Some(sha256))
        .then_some(())
        .ok_or(StorageError::BadDigest)
}

fn hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Lowercase hex of a finished hasher; shared with downloaders that track their own digest.
pub fn finish_hex(hasher: Sha256) -> String {
    hex(hasher)
}
