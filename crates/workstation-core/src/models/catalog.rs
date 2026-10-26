// SPDX-License-Identifier: GPL-3.0-or-later
//! Catalog identifiers and normalization of raw repository data into
//! Family → Release → Artifact. Pure and offline; network access lives in
//! `workstation-runtimes`.

use super::{
    artifact::{
        ArtifactFormat, ModelArtifact, ShardInfo, quantization_from_filename, shard_from_filename,
        shard_group_prefix, valid_remote_filename,
    },
    family::derive_family,
    release::{GatedAccess, ModelRelease},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A Hugging Face `owner/name` repository id that is safe to put in a URL path.
pub fn valid_repo_id(id: &str) -> bool {
    let part = |text: &str| {
        (1..=96).contains(&text.len())
            && text.as_bytes()[0].is_ascii_alphanumeric()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    id.split_once('/')
        .is_some_and(|(owner, name)| part(owner) && part(name))
}

/// A 40-character lowercase hexadecimal commit id.
pub fn valid_revision(revision: &str) -> bool {
    is_lower_hex(revision, 40)
}

/// Canonical lowercase 64-character SHA-256, or `None`.
pub fn canonical_sha256(digest: &str) -> Option<String> {
    let lower = digest.to_ascii_lowercase();
    is_lower_hex(&lower, 64).then_some(lower)
}

/// Stable identity of an artifact at one revision. `:` cannot occur in file names
/// that pass validation, so the separators are unambiguous.
pub fn artifact_id(repo_id: &str, revision: &str, filename: &str) -> String {
    format!("hf:{repo_id}@{revision}:{filename}")
}

/// A repository as listed by a search; carries no file information.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSummary {
    pub repo_id: String,
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    pub gated: GatedAccess,
}

/// One page of search results and the opaque cursor for the next page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPage {
    pub repos: Vec<RepoSummary>,
    pub next_cursor: Option<String>,
}

/// One file as reported by the repository detail endpoint.
#[derive(Clone, Debug)]
pub struct RawSibling {
    pub filename: String,
    pub size: Option<u64>,
    pub lfs_sha256: Option<String>,
}

/// Repository detail at a pinned revision, before validation.
#[derive(Clone, Debug)]
pub struct RawRepoDetail {
    pub repo_id: String,
    pub revision: String,
    pub license: Option<String>,
    pub gated: GatedAccess,
    pub base_model: Option<String>,
    pub siblings: Vec<RawSibling>,
}

/// Why a repository could not be normalized at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogError {
    InvalidRepoId,
    InvalidRevision,
    /// An artifact's identity, name, or checksum is not canonical, or two artifacts collide.
    InvalidArtifact,
}

/// Check a release the way normalization would have built it. Used before caching and
/// after reading a cached row, so a damaged or hand-built release never reaches a URL
/// or a file path.
pub fn validate_release(release: &ModelRelease) -> Result<(), CatalogError> {
    if !valid_repo_id(&release.repo_id) {
        return Err(CatalogError::InvalidRepoId);
    }
    if !valid_revision(&release.revision) {
        return Err(CatalogError::InvalidRevision);
    }
    let mut seen = std::collections::BTreeSet::new();
    for artifact in &release.artifacts {
        let canonical_hash = artifact
            .sha256
            .as_deref()
            .is_none_or(|sha| canonical_sha256(sha).as_deref() == Some(sha));
        let ok = artifact.repo_id == release.repo_id
            && artifact.revision == release.revision
            && valid_remote_filename(&artifact.filename)
            && artifact.filename.to_ascii_lowercase().ends_with(".gguf")
            && artifact.artifact_id
                == artifact_id(&release.repo_id, &release.revision, &artifact.filename)
            && canonical_hash
            && (artifact.shards.is_none() || artifact.sha256.is_none())
            && seen.insert(artifact.artifact_id.as_str());
        if !ok {
            return Err(CatalogError::InvalidArtifact);
        }
    }
    Ok(())
}

fn facts(sibling: &RawSibling) -> (Option<u64>, Option<String>) {
    (
        sibling.size,
        sibling.lfs_sha256.as_deref().and_then(canonical_sha256),
    )
}

/// One sibling per file name. Identical repeats collapse; conflicting repeats are
/// all dropped and counted, because no single answer can be trusted.
fn unique_by_name(siblings: Vec<RawSibling>, dropped: &mut u32) -> Vec<RawSibling> {
    let mut by_name: BTreeMap<String, Vec<RawSibling>> = BTreeMap::new();
    for sibling in siblings {
        by_name
            .entry(sibling.filename.clone())
            .or_default()
            .push(sibling);
    }
    let mut unique = Vec::new();
    for (_, mut copies) in by_name {
        if copies.iter().all(|c| facts(c) == facts(&copies[0])) {
            unique.push(copies.swap_remove(0));
        } else {
            *dropped = dropped.saturating_add(u32::try_from(copies.len()).unwrap_or(u32::MAX));
        }
    }
    unique
}

const OTHER_WEIGHT_EXTENSIONS: &[&str] = &[".safetensors", ".bin", ".pt", ".pth", ".onnx"];

/// Validate and normalize one repository. Invalid files are dropped and counted;
/// shard groups become one artifact; missing facts stay `None`.
pub fn normalize_release(raw: RawRepoDetail) -> Result<ModelRelease, CatalogError> {
    if !valid_repo_id(&raw.repo_id) {
        return Err(CatalogError::InvalidRepoId);
    }
    if !valid_revision(&raw.revision) {
        return Err(CatalogError::InvalidRevision);
    }
    let family = derive_family(raw.base_model.as_deref(), &raw.repo_id);
    let mut dropped_invalid = 0u32;
    let mut other_weight_files = 0u32;
    let mut singles = Vec::new();
    let mut groups: BTreeMap<(String, u16), Vec<(u16, RawSibling)>> = BTreeMap::new();

    for sibling in raw.siblings {
        let lower = sibling.filename.to_ascii_lowercase();
        if !lower.ends_with(".gguf") {
            if OTHER_WEIGHT_EXTENSIONS
                .iter()
                .any(|ext| lower.ends_with(ext))
            {
                other_weight_files = other_weight_files.saturating_add(1);
            }
            continue;
        }
        if !valid_remote_filename(&sibling.filename) {
            dropped_invalid = dropped_invalid.saturating_add(1);
            continue;
        }
        match (
            shard_from_filename(&sibling.filename),
            shard_group_prefix(&sibling.filename),
        ) {
            (Some((index, count)), Some(prefix)) => groups
                .entry((prefix.to_owned(), count))
                .or_default()
                .push((index, sibling)),
            _ => singles.push(sibling),
        }
    }

    let singles = unique_by_name(singles, &mut dropped_invalid);
    let mut artifacts: Vec<ModelArtifact> = singles
        .into_iter()
        .map(|sibling| {
            let sha256 = sibling.lfs_sha256.as_deref().and_then(canonical_sha256);
            ModelArtifact {
                artifact_id: artifact_id(&raw.repo_id, &raw.revision, &sibling.filename),
                quantization_hint: quantization_from_filename(&sibling.filename),
                repo_id: raw.repo_id.clone(),
                revision: raw.revision.clone(),
                filename: sibling.filename,
                format: ArtifactFormat::Gguf,
                size_bytes: sibling.size,
                sha256,
                shards: None,
            }
        })
        .collect();

    for ((_, count), mut parts) in groups {
        parts.sort_by_key(|(index, _)| *index);
        let conflicting = parts
            .windows(2)
            .any(|w| w[0].0 == w[1].0 && facts(&w[0].1) != facts(&w[1].1));
        if conflicting {
            dropped_invalid =
                dropped_invalid.saturating_add(u32::try_from(parts.len()).unwrap_or(u32::MAX));
            continue;
        }
        parts.dedup_by_key(|(index, _)| *index);
        let complete = parts.len() == usize::from(count);
        // The first part names the group; the total is known only for a full set.
        let size_bytes = complete
            .then(|| {
                parts
                    .iter()
                    .try_fold(0u64, |total, (_, s)| total.checked_add(s.size?))
            })
            .flatten();
        let first = &parts[0].1;
        artifacts.push(ModelArtifact {
            artifact_id: artifact_id(&raw.repo_id, &raw.revision, &first.filename),
            quantization_hint: quantization_from_filename(&first.filename),
            repo_id: raw.repo_id.clone(),
            revision: raw.revision.clone(),
            filename: first.filename.clone(),
            format: ArtifactFormat::Gguf,
            size_bytes,
            sha256: None,
            shards: Some(ShardInfo { count, complete }),
        });
    }
    artifacts.sort_by(|a, b| a.filename.cmp(&b.filename));

    Ok(ModelRelease {
        repo_id: raw.repo_id,
        revision: raw.revision,
        family,
        license: raw.license,
        gated: raw.gated,
        artifacts,
        dropped_invalid,
        other_weight_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";
    const SHA: &str = "65a8000000000000000000000000000000000000000000000000000000000000";

    fn sibling(name: &str, size: Option<u64>, sha: Option<&str>) -> RawSibling {
        RawSibling {
            filename: name.into(),
            size,
            lfs_sha256: sha.map(str::to_owned),
        }
    }
    fn detail(siblings: Vec<RawSibling>) -> RawRepoDetail {
        RawRepoDetail {
            repo_id: "bartowski/Qwen2.5-Coder-7B-Instruct-GGUF".into(),
            revision: REV.into(),
            license: Some("apache-2.0".into()),
            gated: GatedAccess::Open,
            base_model: Some("Qwen/Qwen2.5-Coder-7B-Instruct".into()),
            siblings,
        }
    }

    #[test]
    fn identifiers_are_validated() {
        assert!(valid_repo_id("a/b"));
        assert!(valid_repo_id("Qwen/Qwen2.5-Coder-7B_v1"));
        for bad in [
            "", "a", "a/", "/b", "a/b/c", "a/.b", "a/b c", "a/b?x", "../b", "a/ü",
        ] {
            assert!(!valid_repo_id(bad), "{bad}");
        }
        assert!(!valid_repo_id(&format!("a/{}", "x".repeat(97))));
        assert!(valid_revision(REV));
        assert!(!valid_revision("main"));
        assert!(!valid_revision(&REV.to_uppercase()));
        assert_eq!(canonical_sha256(&SHA.to_uppercase()).as_deref(), Some(SHA));
        assert_eq!(canonical_sha256("abc"), None);
    }

    #[test]
    fn single_files_keep_unknowns_unknown() {
        let release = normalize_release(detail(vec![
            sibling("m-Q4_K_M.gguf", Some(4_683_073_536), Some(SHA)),
            sibling("m-Q8_0.gguf", None, None),
            sibling("README.md", Some(1), None),
        ]))
        .unwrap();
        assert_eq!(release.family.name, "Qwen2.5-Coder");
        assert_eq!(release.artifacts.len(), 2);
        let q4 = &release.artifacts[0];
        assert_eq!(q4.quantization_hint.as_deref(), Some("Q4_K_M"));
        assert_eq!(q4.sha256.as_deref(), Some(SHA));
        assert_eq!(
            q4.artifact_id,
            format!("hf:bartowski/Qwen2.5-Coder-7B-Instruct-GGUF@{REV}:m-Q4_K_M.gguf")
        );
        let q8 = &release.artifacts[1];
        assert_eq!((q8.size_bytes, q8.sha256.as_deref()), (None, None));
    }

    #[test]
    fn shards_group_into_one_artifact_without_a_checksum() {
        let release = normalize_release(detail(vec![
            sibling("big-00002-of-00002.gguf", Some(5), Some(SHA)),
            sibling("big-00001-of-00002.gguf", Some(7), Some(SHA)),
            sibling("part-00001-of-00003.gguf", Some(1), None),
        ]))
        .unwrap();
        let big = release
            .artifacts
            .iter()
            .find(|a| a.filename == "big-00001-of-00002.gguf")
            .unwrap();
        assert_eq!(
            big.shards,
            Some(ShardInfo {
                count: 2,
                complete: true
            })
        );
        assert_eq!((big.size_bytes, big.sha256.as_deref()), (Some(12), None));
        let part = release
            .artifacts
            .iter()
            .find(|a| a.filename.starts_with("part"))
            .unwrap();
        assert_eq!(
            part.shards,
            Some(ShardInfo {
                count: 3,
                complete: false
            })
        );
        assert_eq!(part.size_bytes, None);
    }

    #[test]
    fn hostile_and_foreign_files_are_counted_not_offered() {
        let release = normalize_release(detail(vec![
            sibling("../evil.gguf", Some(1), Some(SHA)),
            sibling("a:b.gguf", Some(1), Some(SHA)),
            sibling("sub/ok.gguf", Some(1), Some(SHA)),
            sibling("model.safetensors", Some(1), None),
            sibling("pytorch_model.bin", Some(1), None),
        ]))
        .unwrap();
        assert_eq!(release.dropped_invalid, 2);
        assert_eq!(release.other_weight_files, 2);
        assert_eq!(release.artifacts.len(), 1);
        assert_eq!(release.artifacts[0].filename, "sub/ok.gguf");
    }

    #[test]
    fn invalid_repo_or_revision_fails_whole_release() {
        let mut raw = detail(vec![]);
        raw.revision = "main".into();
        assert_eq!(
            normalize_release(raw).unwrap_err(),
            CatalogError::InvalidRevision
        );
        let mut raw = detail(vec![]);
        raw.repo_id = "no-owner".into();
        assert_eq!(
            normalize_release(raw).unwrap_err(),
            CatalogError::InvalidRepoId
        );
    }

    #[test]
    fn duplicate_names_collapse_or_are_dropped() {
        let release = normalize_release(detail(vec![
            sibling("same.gguf", Some(10), Some(SHA)),
            sibling("same.gguf", Some(10), Some(&SHA.to_uppercase())),
            sibling("clash.gguf", Some(10), Some(SHA)),
            sibling("clash.gguf", Some(11), Some(SHA)),
            sibling("s-00001-of-00002.gguf", Some(1), None),
            sibling("s-00001-of-00002.gguf", Some(2), None),
            sibling("s-00002-of-00002.gguf", Some(3), None),
        ]))
        .unwrap();
        let names: Vec<_> = release
            .artifacts
            .iter()
            .map(|a| a.filename.as_str())
            .collect();
        assert_eq!(names, ["same.gguf"]);
        assert_eq!(release.dropped_invalid, 5);
        assert_eq!(validate_release(&release), Ok(()));
    }

    #[test]
    fn validation_rejects_forged_artifacts() {
        let good = normalize_release(detail(vec![sibling("m.gguf", Some(1), Some(SHA))])).unwrap();
        assert_eq!(validate_release(&good), Ok(()));
        let mutate: [fn(&mut ModelRelease); 6] = [
            |r| r.artifacts[0].filename = "../escape.gguf".into(),
            |r| r.artifacts[0].sha256 = Some("../outside".into()),
            |r| r.artifacts[0].revision = "main".into(),
            |r| r.artifacts[0].artifact_id = "hf:other".into(),
            |r| r.artifacts.push(r.artifacts[0].clone()),
            |r| r.artifacts[0].sha256 = Some(SHA.to_uppercase()),
        ];
        for change in mutate {
            let mut bad = good.clone();
            change(&mut bad);
            assert_eq!(validate_release(&bad), Err(CatalogError::InvalidArtifact));
        }
    }

    #[test]
    fn gated_values_map_from_the_api() {
        use serde_json::json;
        assert_eq!(GatedAccess::from_api(&json!(false)), GatedAccess::Open);
        assert_eq!(
            GatedAccess::from_api(&json!("auto")),
            GatedAccess::Automatic
        );
        assert_eq!(GatedAccess::from_api(&json!("manual")), GatedAccess::Manual);
        assert_eq!(GatedAccess::from_api(&json!(true)), GatedAccess::Unknown);
        assert_eq!(
            GatedAccess::from_stored(GatedAccess::Manual.as_str()),
            GatedAccess::Manual
        );
    }
}
