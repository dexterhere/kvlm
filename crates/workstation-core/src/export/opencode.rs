// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenCode `opencode.json` provider entry for a launch profile.

use serde_json::{Value, json};

use crate::runtime::profile::{LOOPBACK, LaunchProfile};

/// A provider fragment to merge by hand; the application never edits OpenCode's file.
pub fn provider_fragment(profile: &LaunchProfile) -> Result<String, String> {
    profile.validate()?;
    let value: Value = json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            "llamacpp": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "llama.cpp (local)",
                "options": { "baseURL": format!("http://{LOOPBACK}:{}/v1", profile.port) },
                "models": {
                    profile.name.as_str(): {
                        "name": profile.name,
                        "limit": { "context": profile.context_length, "output": 4096 },
                    }
                }
            }
        }
    });
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}
