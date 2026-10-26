// SPDX-License-Identifier: GPL-3.0-or-later
//! Normalized model family: the grouping above a concrete release.

use super::artifact::quantization_from_filename;
use serde::{Deserialize, Serialize};

/// How the family name was found, so the interface can say it was derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FamilySource {
    /// From the repository's declared base model.
    BaseModel,
    /// From the repository name, with format and size suffixes removed.
    RepoName,
    /// Nothing usable was found.
    Ungrouped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFamily {
    pub name: String,
    pub publisher: Option<String>,
    pub source: FamilySource,
}

pub const UNGROUPED: &str = "Ungrouped";

/// Derive a family from a base model id (`org/name`) or, failing that, the repo id.
/// `Qwen2.5-Coder-7B-Instruct-GGUF` becomes `Qwen2.5-Coder`. This is a heuristic and
/// is labelled as derived.
pub fn derive_family(base_model: Option<&str>, repo_id: &str) -> ModelFamily {
    let (source_id, source) = match base_model.filter(|id| id.contains('/')) {
        Some(id) => (id, FamilySource::BaseModel),
        None => (repo_id, FamilySource::RepoName),
    };
    let (publisher, name) = source_id.split_once('/').unwrap_or(("", source_id));
    let name = family_name(name);
    if name.is_empty() {
        return ModelFamily {
            name: UNGROUPED.into(),
            publisher: None,
            source: FamilySource::Ungrouped,
        };
    }
    ModelFamily {
        name,
        publisher: (!publisher.is_empty()).then(|| publisher.to_owned()),
        source,
    }
}

fn family_name(name: &str) -> String {
    let trimmed = ["-GGUF", "_GGUF", ".gguf", "-gguf"]
        .iter()
        .fold(name, |acc, suffix| acc.strip_suffix(suffix).unwrap_or(acc));
    let mut segments: Vec<&str> = trimmed.split('-').collect();
    // `Foo-Q4_K_M` and `Foo-Q8_0` are one family: drop trailing quantization labels.
    while segments.len() > 1
        && segments.last().is_some_and(|last| {
            quantization_from_filename(last).is_some_and(|q| q.eq_ignore_ascii_case(last))
        })
    {
        segments.pop();
    }
    let cut = segments
        .iter()
        .skip(1)
        .position(|segment| is_size_token(segment))
        .map(|index| index + 1);
    match cut {
        Some(cut) => segments[..cut].join("-"),
        None => segments.join("-"),
    }
}

/// `7B`, `1.5B`, `135M`, `30B` — a parameter-count marker that ends the family name.
fn is_size_token(segment: &str) -> bool {
    let Some(number) = segment
        .strip_suffix(['B', 'b', 'M', 'm'])
        .filter(|n| !n.is_empty())
    else {
        return false;
    };
    let mut dots = 0;
    number.bytes().all(|b| {
        dots += usize::from(b == b'.');
        b.is_ascii_digit() || (b == b'.' && dots <= 1)
    }) && number.as_bytes()[0].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_comes_from_base_model_then_repo_name() {
        let family = derive_family(Some("Qwen/Qwen2.5-Coder-7B-Instruct"), "bartowski/x-GGUF");
        assert_eq!(family.name, "Qwen2.5-Coder");
        assert_eq!(family.publisher.as_deref(), Some("Qwen"));
        assert_eq!(family.source, FamilySource::BaseModel);

        let family = derive_family(None, "bartowski/Qwen2.5-Coder-7B-Instruct-GGUF");
        assert_eq!(family.name, "Qwen2.5-Coder");
        assert_eq!(family.source, FamilySource::RepoName);

        assert_eq!(derive_family(None, "a/Qwen3-30B-A3B-GGUF").name, "Qwen3");
        assert_eq!(
            derive_family(None, "a/TinyLlama-1.1B-Chat-GGUF").name,
            "TinyLlama"
        );
        assert_eq!(derive_family(None, "a/phi-3-mini-GGUF").name, "phi-3-mini");
        assert_eq!(derive_family(None, "o/Foo-Q4_K_M-GGUF").name, "Foo");
        assert_eq!(derive_family(None, "o/Foo-Q8_0-GGUF").name, "Foo");
    }

    #[test]
    fn empty_names_are_ungrouped() {
        let family = derive_family(None, "a/-GGUF");
        assert_eq!(
            (family.name.as_str(), family.source),
            (UNGROUPED, FamilySource::Ungrouped)
        );
        // A base model without an owner is not trusted.
        assert_eq!(
            derive_family(Some("noslash"), "a/Foo-1B").source,
            FamilySource::RepoName
        );
    }

    #[test]
    fn size_tokens_need_digits() {
        for yes in ["7B", "1.5B", "135M", "30b"] {
            assert!(is_size_token(yes), "{yes}");
        }
        for no in ["B", "Base", "v1B", "1.2.3B", ".5B", "7"] {
            assert!(!is_size_token(no), "{no}");
        }
    }
}
