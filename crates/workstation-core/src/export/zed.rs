// SPDX-License-Identifier: GPL-3.0-or-later
//! Zed `settings.json` OpenAI-compatible provider entry for a launch profile.

use serde_json::{Value, json};

use crate::runtime::profile::{LOOPBACK, LaunchProfile};

/// A `language_models` fragment to merge by hand; the application never edits Zed's file.
pub fn settings_fragment(profile: &LaunchProfile) -> Result<String, String> {
    profile.validate()?;
    let value: Value = json!({
        "language_models": {
            "openai_compatible": {
                "llama.cpp (local)": {
                    "api_url": format!("http://{LOOPBACK}:{}/v1", profile.port),
                    "available_models": [{
                        "name": profile.name,
                        "max_tokens": profile.context_length,
                        "max_output_tokens": 4096,
                        "max_completion_tokens": 4096,
                    }],
                }
            }
        }
    });
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}
