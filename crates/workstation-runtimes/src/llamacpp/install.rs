// SPDX-License-Identifier: GPL-3.0-or-later
//! Downloading, verifying and publishing a portable llama.cpp build without root.
//!
//! Every network step follows an explicit user action. A build is published only after its
//! archives matched the SHA-256 GitHub published for them, extracted under the safety rules
//! in `extract`, and the server binary ran `--version` in a cleared environment. Failure at
//! any step leaves nothing behind but a removed staging directory.

use super::{
    extract::{self, ExtractError, LIMITS},
    release::{self, Asset, Backend, Candidate},
};
use reqwest::{Client, StatusCode, Url, header, redirect::Policy};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use workstation_core::{
    models::download::finish_hex,
    runtime::{profile::valid_name, random_hex},
};

const API: &str = "https://api.github.com";
const DOWNLOADS: &str = "https://github.com/ggml-org/llama.cpp/releases/download";
const ALLOWED_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_LISTING: usize = 8 * 1024 * 1024;
const MAX_HOPS: usize = 4;
const RESERVE: u64 = 1 << 30;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    Offline,
    Timeout,
    RateLimited,
    BadResponse,
    TooLarge,
    /// A downloaded archive did not match its published digest or size.
    Integrity,
    NothingToInstall,
    AlreadyInstalled,
    /// Another instance is installing or removing a build.
    Busy,
    Cancelled,
    NotEnoughSpace,
    Archive(ExtractError),
    /// The build unpacked but `llama-server` would not run on this machine.
    WontRun,
    Unsafe(&'static str),
    Io,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Archive(error) => return error.fmt(f),
            Self::Unsafe(why) => return write!(f, "Refused: {why}."),
            _ => {}
        }
        f.write_str(match self {
            Self::Offline => "Cannot reach GitHub. Check the network connection and retry.",
            Self::Timeout => "GitHub did not answer in time. Retry shortly.",
            Self::RateLimited => "GitHub is rate limiting requests. Wait and retry.",
            Self::BadResponse => "GitHub sent a response the application cannot use.",
            Self::TooLarge => "GitHub sent more data than the application accepts.",
            Self::Integrity => "The download did not match its published checksum and was discarded.",
            Self::NothingToInstall => "No verified llama.cpp build is available for this machine.",
            Self::AlreadyInstalled => "This build is already installed.",
            Self::Busy => "Another install or removal is in progress.",
            Self::Cancelled => "The install was cancelled.",
            Self::NotEnoughSpace => "Not enough free disk space for this build.",
            Self::WontRun => "The build unpacked but does not run on this system (missing libraries or an incompatible system).",
            Self::Io => "Cannot write the runtime files. Check free disk space and permissions.",
            Self::Archive(_) | Self::Unsafe(_) => unreachable!(),
        })
    }
}
impl std::error::Error for InstallError {}

impl From<ExtractError> for InstallError {
    fn from(error: ExtractError) -> Self {
        Self::Archive(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub archive: usize,
    pub archives: usize,
    pub done: u64,
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub tag: String,
    pub backend: Backend,
    pub dir_name: String,
    /// SHA-256 of the main (first) archive.
    pub sha256: String,
}

pub struct Installer {
    client: Client,
    api: Url,
    downloads: Url,
    strict: bool,
}

impl Installer {
    pub fn new() -> Result<Self, InstallError> {
        Self::build(API, DOWNLOADS, true)
    }

    #[cfg(test)]
    pub(crate) fn for_test(port: u16) -> Self {
        let base = format!("http://127.0.0.1:{port}");
        Self::build(&base, &format!("{base}/dl"), false).unwrap()
    }

    fn build(api: &str, downloads: &str, strict: bool) -> Result<Self, InstallError> {
        let mut builder = Client::builder();
        if strict {
            builder = builder.dns_resolver(Arc::new(crate::huggingface::PublicOnly));
        } else {
            builder = builder.no_proxy();
        }
        let client = builder
            .use_rustls_tls()
            .https_only(strict)
            .redirect(Policy::none())
            .referer(false)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .user_agent(concat!("local-ai-workstation/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| InstallError::BadResponse)?;
        Ok(Self {
            client,
            api: Url::parse(api).map_err(|_| InstallError::BadResponse)?,
            downloads: Url::parse(downloads).map_err(|_| InstallError::BadResponse)?,
            strict,
        })
    }

    /// Verified builds for the wanted backends from the newest suitable release.
    pub async fn list_candidates(
        &self,
        wanted: &[Backend],
    ) -> Result<Vec<Candidate>, InstallError> {
        let mut url = self.api.clone();
        url.set_path("/repos/ggml-org/llama.cpp/releases");
        url.set_query(Some("per_page=20"));
        let response = self
            .client
            .get(url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(net_error)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
                return Err(InstallError::RateLimited);
            }
            _ => return Err(InstallError::BadResponse),
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_LISTING as u64)
        {
            return Err(InstallError::TooLarge);
        }
        let mut body = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(net_error)? {
            if body.len() + chunk.len() > MAX_LISTING {
                return Err(InstallError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        release::select(&body, wanted).map_err(|_| InstallError::BadResponse)
    }

    /// Download, verify, extract and publish `candidate` under `root` (the llama.cpp
    /// install directory). `cancel` is checked between network chunks.
    pub async fn install(
        &self,
        root: &Path,
        candidate: &Candidate,
        cancel: &AtomicBool,
        mut progress: impl FnMut(Progress),
    ) -> Result<Installed, InstallError> {
        let name = candidate.dir_name();
        if !release::valid_tag(&candidate.tag)
            || !valid_name(&name)
            || candidate.archives.is_empty()
        {
            return Err(InstallError::Unsafe("invalid build identity"));
        }
        prepare_root(root)?;
        let _lock = RootLock::acquire(root)?;
        remove_stale_stages(root);
        let target = root.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(InstallError::AlreadyInstalled);
        }
        let total = candidate.total_size();
        check_space(root, total.saturating_mul(3))?;

        let stage = Stage::create(root)?;
        let content = stage.path.join("content");
        DirBuilder::new()
            .mode(0o700)
            .create(&content)
            .map_err(|_| InstallError::Io)?;
        let mut done_before = 0;
        for (index, asset) in candidate.archives.iter().enumerate() {
            let archive_path = stage.path.join(format!("archive-{index}.tar.gz"));
            self.download(&candidate.tag, asset, &archive_path, cancel, &mut |done| {
                progress(Progress {
                    archive: index,
                    archives: candidate.archives.len(),
                    done: done_before + done,
                    total,
                });
            })
            .await?;
            let (path, dest) = (archive_path.clone(), content.clone());
            tokio::task::spawn_blocking(move || {
                let file = File::open(&path).map_err(|_| InstallError::Io)?;
                extract::extract_tar_gz(file, &dest, LIMITS)?;
                fs::remove_file(&path).map_err(|_| InstallError::Io)
            })
            .await
            .map_err(|_| InstallError::Io)??;
            done_before += asset.size;
        }
        let server = content.join("llama-server");
        tokio::task::spawn_blocking({
            let (server, content) = (server.clone(), content.clone());
            move || verify_runs(&server, &content)
        })
        .await
        .map_err(|_| InstallError::Io)??;

        publish(&content, &target)?;
        File::open(root)
            .and_then(|d| d.sync_all())
            .map_err(|_| InstallError::Io)?;
        Ok(Installed {
            tag: candidate.tag.clone(),
            backend: candidate.backend,
            dir_name: name,
            sha256: candidate.archives[0].sha256.clone(),
        })
    }

    /// Stream one archive to `path`, hashing as it arrives.
    async fn download(
        &self,
        tag: &str,
        asset: &Asset,
        path: &Path,
        cancel: &AtomicBool,
        report: &mut impl FnMut(u64),
    ) -> Result<(), InstallError> {
        let mut url = self.downloads.clone();
        url.path_segments_mut()
            .map_err(|_| InstallError::BadResponse)?
            .pop_if_empty()
            .push(tag)
            .push(&asset.name);
        let mut response = None;
        for _ in 0..=MAX_HOPS {
            self.check_host(&url)?;
            let reply = self
                .client
                .get(url.clone())
                .send()
                .await
                .map_err(net_error)?;
            if reply.status().is_redirection() {
                let location = reply
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(InstallError::BadResponse)?;
                url = url.join(location).map_err(|_| InstallError::BadResponse)?;
                continue;
            }
            response = Some(reply);
            break;
        }
        let mut response = response.ok_or(InstallError::BadResponse)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
                return Err(InstallError::RateLimited);
            }
            _ => return Err(InstallError::BadResponse),
        }
        if response.content_length().is_some_and(|n| n != asset.size) {
            return Err(InstallError::Integrity);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| InstallError::Io)?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut last = Instant::now();
        while let Some(chunk) = response.chunk().await.map_err(net_error)? {
            if cancel.load(Ordering::Relaxed) {
                return Err(InstallError::Cancelled);
            }
            done += chunk.len() as u64;
            if done > asset.size {
                return Err(InstallError::Integrity);
            }
            file.write_all(&chunk).map_err(|_| InstallError::Io)?;
            hasher.update(&chunk);
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                report(done);
            }
        }
        report(done);
        if done != asset.size || finish_hex(hasher) != asset.sha256 {
            return Err(InstallError::Integrity);
        }
        file.sync_all().map_err(|_| InstallError::Io)
    }

    fn check_host(&self, url: &Url) -> Result<(), InstallError> {
        if !self.strict {
            return (url.host_str() == self.downloads.host_str()
                && url.port() == self.downloads.port())
            .then_some(())
            .ok_or(InstallError::Unsafe("download left the allowed host"));
        }
        let ok = url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.port(), None | Some(443))
            && url.host_str().is_some_and(|h| ALLOWED_HOSTS.contains(&h));
        ok.then_some(())
            .ok_or(InstallError::Unsafe("download left the allowed hosts"))
    }
}

fn net_error(error: reqwest::Error) -> InstallError {
    if error.is_timeout() {
        InstallError::Timeout
    } else if error.is_connect() {
        InstallError::Offline
    } else {
        InstallError::BadResponse
    }
}

/// Create the install root privately and refuse one that is a link or not ours.
pub fn prepare_root(root: &Path) -> Result<(), InstallError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|_| InstallError::Io)?;
    let meta = fs::symlink_metadata(root).map_err(|_| InstallError::Io)?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o022 != 0
    {
        return Err(InstallError::Unsafe("install directory is not private"));
    }
    Ok(())
}

fn check_space(root: &Path, needed: u64) -> Result<(), InstallError> {
    let stat = rustix::fs::statvfs(root).map_err(|_| InstallError::Io)?;
    let free = stat.f_bavail.saturating_mul(stat.f_frsize);
    if free < needed.saturating_add(RESERVE) {
        return Err(InstallError::NotEnoughSpace);
    }
    Ok(())
}

/// Advisory lock held for the length of an install or removal, across instances.
pub struct RootLock {
    _file: File,
}

impl RootLock {
    pub fn acquire(root: &Path) -> Result<Self, InstallError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(root.join(".lock"))
            .map_err(|_| InstallError::Io)?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| InstallError::Busy)?;
        Ok(Self { _file: file })
    }
}

struct Stage {
    path: PathBuf,
}

impl Stage {
    fn create(root: &Path) -> Result<Self, InstallError> {
        let id = random_hex(8).map_err(|_| InstallError::Io)?;
        let path = root.join(format!(".stage-{id}"));
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|_| InstallError::Io)?;
        Ok(Self { path })
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Staging directories left by a crash. Called only while holding the root lock.
fn remove_stale_stages(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".stage-")
            && entry.file_type().is_ok_and(|t| t.is_dir())
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// `llama-server` must be a regular executable file that runs and exits cleanly.
fn verify_runs(server: &Path, libs: &Path) -> Result<(), InstallError> {
    let meta = fs::symlink_metadata(server).map_err(|_| InstallError::WontRun)?;
    if !meta.is_file() || meta.permissions().mode() & 0o100 == 0 {
        return Err(InstallError::WontRun);
    }
    let mut child = Command::new(server)
        .arg("--version")
        .env_clear()
        .env("LD_LIBRARY_PATH", libs)
        .current_dir(libs)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| InstallError::WontRun)?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => return Err(InstallError::WontRun),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(InstallError::WontRun);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn publish(from: &Path, to: &Path) -> Result<(), InstallError> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| match error {
        rustix::io::Errno::EXIST | rustix::io::Errno::NOTEMPTY => InstallError::AlreadyInstalled,
        _ => InstallError::Io,
    })
}

/// Remove an installed build this application published. `in_use` names the directory a
/// managed process currently runs from; that one is never removed. The caller has already
/// confirmed `dir_name` is one of its own install records.
pub fn remove_install(
    root: &Path,
    dir_name: &str,
    in_use: Option<&str>,
) -> Result<(), InstallError> {
    if !valid_name(dir_name) || dir_name.starts_with('.') {
        return Err(InstallError::Unsafe("invalid build name"));
    }
    if in_use == Some(dir_name) {
        return Err(InstallError::Unsafe("this build is running"));
    }
    let _lock = RootLock::acquire(root)?;
    let target = root.join(dir_name);
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&target).map_err(|_| InstallError::Io),
        Ok(_) => Err(InstallError::Unsafe("not a build directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(InstallError::Io),
    }
}

#[cfg(test)]
mod tests;
