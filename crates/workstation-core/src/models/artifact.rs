// SPDX-License-Identifier: GPL-3.0-or-later
//! Concrete downloadable artifacts, plus runtime manifest identity.

use serde::{Deserialize, Serialize};

/// Canonical Ollama manifest identity; this is not a GGUF content digest.
pub fn canonical_manifest_digest(digest: &str) -> Option<String> {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("sha256:{}", hex.to_ascii_lowercase()))
}

/// Weight file format. Only GGUF is supported in M1; others are counted, not listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactFormat {
    Gguf,
}

/// Multi-file GGUF group. Fit and download stay unavailable until sharding is validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardInfo {
    pub count: u16,
    /// Every part from 1 to `count` was listed.
    pub complete: bool,
}

/// One downloadable weight bundle at an immutable repository revision.
///
/// Missing facts stay `None`; an artifact without a checksum is listed but not
/// downloadable, and `quantization_hint` comes from the file name until the GGUF
/// header confirms it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub artifact_id: String,
    pub repo_id: String,
    pub revision: String,
    pub filename: String,
    pub format: ArtifactFormat,
    pub quantization_hint: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub shards: Option<ShardInfo>,
}

/// Known GGUF quantization labels, longest first so `Q4_K_M` wins over `Q4_K`.
const QUANTIZATIONS: &[&str] = &[
    "IQ2_XXS", "IQ3_XXS", "Q4_K_XL", "Q3_K_XL", "Q2_K_L", "Q3_K_L", "Q3_K_M", "Q3_K_S", "Q4_K_L",
    "Q4_K_M", "Q4_K_S", "Q5_K_L", "Q5_K_M", "Q5_K_S", "Q6_K_L", "IQ1_M", "IQ1_S", "IQ2_M", "IQ2_S",
    "IQ2_XS", "IQ3_M", "IQ3_S", "IQ3_XS", "IQ4_NL", "IQ4_XS", "MXFP4", "TQ1_0", "TQ2_0", "BF16",
    "F16", "F32", "Q2_K", "Q3_K", "Q4_0", "Q4_1", "Q4_K", "Q5_0", "Q5_1", "Q5_K", "Q6_K", "Q8_0",
];

/// Quantization label found in a file name, such as `Q4_K_M`. A hint, not a fact.
pub fn quantization_from_filename(filename: &str) -> Option<String> {
    let name = filename
        .rsplit('/')
        .next()
        .unwrap_or(filename)
        .to_ascii_uppercase();
    let bytes = name.as_bytes();
    let boundary = |index: Option<usize>| index.is_none_or(|i| !bytes[i].is_ascii_alphanumeric());
    let mut best: Option<&str> = None;
    for &label in QUANTIZATIONS {
        let mut from = 0;
        while let Some(found) = name[from..].find(label) {
            let start = from + found;
            let end = start + label.len();
            if boundary(start.checked_sub(1)) && boundary((end < bytes.len()).then_some(end)) {
                if best.is_none_or(|current| label.len() > current.len()) {
                    best = Some(label);
                }
                break;
            }
            from = start + 1;
        }
    }
    best.map(str::to_owned)
}

/// Shard position parsed from `name-00001-of-00004.gguf`, as `(index, count)`.
pub fn shard_from_filename(filename: &str) -> Option<(u16, u16)> {
    let name = filename.rsplit('/').next()?;
    let (stem, extension) = name.split_at_checked(name.len().checked_sub(5)?)?;
    if !extension.eq_ignore_ascii_case(".gguf") {
        return None;
    }
    let (left, count) = stem.rsplit_once("-of-")?;
    let (_, index) = left.rsplit_once('-')?;
    let digits = |text: &str| {
        (text.len() == 5 && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u16>().ok())
            .flatten()
    };
    let (index, count) = (digits(index)?, digits(count)?);
    (index >= 1 && index <= count).then_some((index, count))
}

/// Shared prefix of a shard group (`dir/name` without the `-NNNNN-of-NNNNN.gguf` tail).
pub(crate) fn shard_group_prefix(filename: &str) -> Option<&str> {
    shard_from_filename(filename)?;
    let stem = &filename[..filename.len() - 5];
    let (left, _) = stem.rsplit_once("-of-")?;
    left.rsplit_once('-').map(|(prefix, _)| prefix)
}

/// A remote file name is untrusted. It may name a folder, but never an escape.
pub fn valid_remote_filename(filename: &str) -> bool {
    !filename.is_empty()
        && filename.len() <= 255
        && filename.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && !segment
                    .chars()
                    .any(|c| c.is_control() || c == '\\' || c == ':')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_prefers_the_longest_bounded_label() {
        for (name, expected) in [
            ("Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf", Some("Q4_K_M")),
            ("model-q8_0.gguf", Some("Q8_0")),
            ("sub/model.IQ3_XS.gguf", Some("IQ3_XS")),
            ("model-F16.gguf", Some("F16")),
            ("model-Q4_K.gguf", Some("Q4_K")),
            ("model-UD-Q4_K_XL.gguf", Some("Q4_K_XL")),
            ("model-BF16.gguf", Some("BF16")),
            ("modelF16x.gguf", None),
            ("plain.gguf", None),
        ] {
            assert_eq!(
                quantization_from_filename(name).as_deref(),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn shard_markers_are_strict() {
        assert_eq!(shard_from_filename("m-00001-of-00004.gguf"), Some((1, 4)));
        assert_eq!(
            shard_from_filename("dir/m-00004-of-00004.GGUF"),
            Some((4, 4))
        );
        for bad in [
            "m-0001-of-0004.gguf",
            "m-00005-of-00004.gguf",
            "m-00000-of-00004.gguf",
            "m-00001-of-00004.bin",
            "m.gguf",
        ] {
            assert_eq!(shard_from_filename(bad), None, "{bad}");
        }
        assert_eq!(
            shard_group_prefix("d/m-Q4-00001-of-00002.gguf"),
            Some("d/m-Q4")
        );
    }

    #[test]
    fn hostile_filenames_are_rejected() {
        assert!(valid_remote_filename("Q4_K_M/model-00001-of-00002.gguf"));
        for bad in [
            "",
            "..",
            "../x.gguf",
            "/abs.gguf",
            "a//b.gguf",
            "a/./b.gguf",
            "a\\b.gguf",
            "a:b.gguf",
            "a\0b.gguf",
            "a\nb.gguf",
        ] {
            assert!(!valid_remote_filename(bad), "{bad:?}");
        }
        assert!(!valid_remote_filename(&"a".repeat(256)));
    }
}
