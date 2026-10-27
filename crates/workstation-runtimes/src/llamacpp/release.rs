// SPDX-License-Identifier: GPL-3.0-or-later
//! Choosing a llama.cpp build from the GitHub releases listing. Asset names and download
//! URLs are built here from the tag, never copied from the response, and an asset without
//! a published SHA-256 digest is never offered.

use serde::Deserialize;
use workstation_core::models::catalog::canonical_sha256;

pub const MAX_ARCHIVE: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    Cpu,
    Vulkan,
    Cuda,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
            Self::Cuda => "cuda",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "cpu" => Self::Cpu,
            "vulkan" => Self::Vulkan,
            "cuda" => Self::Cuda,
            _ => return None,
        })
    }
    /// Archive file names for one tag; CUDA also needs the separate runtime bundle.
    fn archive_names(self, tag: &str) -> Vec<String> {
        match self {
            Self::Cpu => vec![format!("llama-{tag}-bin-ubuntu-x64.tar.gz")],
            Self::Vulkan => vec![format!("llama-{tag}-bin-ubuntu-vulkan-x64.tar.gz")],
            Self::Cuda => vec![
                format!("llama-{tag}-bin-ubuntu-cuda-12.8-x64.tar.gz"),
                format!("cudart-llama-{tag}-bin-ubuntu-cuda-12.8-x64.tar.gz"),
            ],
        }
    }
}

/// One archive to download: a name, its published size and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub tag: String,
    pub backend: Backend,
    pub archives: Vec<Asset>,
}

impl Candidate {
    pub fn total_size(&self) -> u64 {
        self.archives.iter().map(|a| a.size).sum()
    }
    /// The directory name this build installs under.
    pub fn dir_name(&self) -> String {
        format!("{}-{}", self.tag, self.backend.as_str())
    }
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    state: String,
}

/// `b` followed by digits, as upstream tags builds.
pub fn valid_tag(tag: &str) -> bool {
    tag.len() <= 12
        && tag
            .strip_prefix('b')
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

fn asset(release: &RawRelease, name: &str) -> Option<Asset> {
    let raw = release.assets.iter().find(|a| a.name == name)?;
    let sha256 = canonical_sha256(raw.digest.as_deref()?.strip_prefix("sha256:")?)?;
    (raw.state == "uploaded" && raw.size > 0 && raw.size <= MAX_ARCHIVE).then(|| Asset {
        name: name.to_owned(),
        size: raw.size,
        sha256,
    })
}

/// The newest release that can offer at least one wanted backend, with every archive of
/// each offered backend present and digest-bearing.
pub fn select(body: &[u8], wanted: &[Backend]) -> Result<Vec<Candidate>, String> {
    let releases: Vec<RawRelease> =
        serde_json::from_slice(body).map_err(|_| "The release list is not valid.".to_string())?;
    for release in releases
        .iter()
        .filter(|r| !r.draft && valid_tag(&r.tag_name))
    {
        let found: Vec<Candidate> = wanted
            .iter()
            .filter_map(|&backend| {
                let archives = backend
                    .archive_names(&release.tag_name)
                    .iter()
                    .map(|name| asset(release, name))
                    .collect::<Option<Vec<_>>>()?;
                Some(Candidate {
                    tag: release.tag_name.clone(),
                    backend,
                    archives,
                })
            })
            .collect();
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

/// Which builds are worth offering on this machine.
pub fn backends_for(vulkan_icd: bool, nvidia_driver_major: Option<u32>) -> Vec<Backend> {
    let mut backends = vec![Backend::Cpu];
    if vulkan_icd {
        backends.push(Backend::Vulkan);
    }
    // CUDA 12.8 needs a driver from the 570 series or newer.
    if nvidia_driver_major.is_some_and(|major| major >= 570) {
        backends.push(Backend::Cuda);
    }
    backends
}

/// Reads the loader and driver files; nothing is executed.
pub fn detect_backends() -> Vec<Backend> {
    let vulkan = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .any(|dir| {
            std::fs::read_dir(dir).is_ok_and(|mut entries| {
                entries.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "json")))
            })
        });
    let driver = std::fs::read_to_string("/proc/driver/nvidia/version")
        .ok()
        .and_then(|text| nvidia_major(&text));
    backends_for(vulkan, driver)
}

/// First dotted version in the kernel module line, e.g. `570.133.07`.
pub fn nvidia_major(text: &str) -> Option<u32> {
    text.lines().next()?.split_whitespace().find_map(|word| {
        let (major, rest) = word.split_once('.')?;
        rest.chars().next()?.is_ascii_digit().then_some(())?;
        major.parse().ok()
    })
}
