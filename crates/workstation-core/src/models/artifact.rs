// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime manifest identity. Catalog artifact services remain future work.

/// Canonical Ollama manifest identity; this is not a GGUF content digest.
pub fn canonical_manifest_digest(digest: &str) -> Option<String> {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("sha256:{}", hex.to_ascii_lowercase()))
}
