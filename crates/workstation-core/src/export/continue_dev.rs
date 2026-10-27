// SPDX-License-Identifier: GPL-3.0-or-later
//! Continue.dev `config.yaml` model entry for a launch profile.

use crate::runtime::profile::{LOOPBACK, LaunchProfile};

/// A `models:` list item. Scalars are JSON double-quoted strings, which are valid YAML, so a
/// name or value cannot break the structure. Pure text; the user decides where it goes.
pub fn model_entry(profile: &LaunchProfile) -> Result<String, String> {
    profile.validate()?;
    let quote = |text: &str| serde_json::to_string(text).map_err(|e| e.to_string());
    Ok(format!(
        "models:\n  - name: {}\n    provider: openai\n    model: {}\n    apiBase: {}\n    apiKey: \"none\"\n    roles:\n      - chat\n",
        quote(&profile.name)?,
        quote(&profile.name)?,
        quote(&format!("http://{LOOPBACK}:{}/v1", profile.port))?,
    ))
}
