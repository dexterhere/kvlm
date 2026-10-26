// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings as requested by a launch profile, as observed from a running llama-server's own
//! command line, and the single-variable clone used for a controlled follow-up.

use super::{Settings, Variable};
use crate::runtime::profile::{CacheType, ExtraFlag, LaunchProfile};

/// What the profile asks the runtime to do.
pub fn requested_settings(profile: &LaunchProfile) -> Settings {
    let mut settings = Settings {
        context_length: Some(profile.context_length),
        gpu_layers: Some(u32::from(profile.gpu_layers)),
        cache_type_k: Some(profile.cache_type_k.flag_value().into()),
        cache_type_v: Some(profile.cache_type_v.flag_value().into()),
        flash_attention: Some(profile.flash_attention),
        ubatch_size: Some(profile.ubatch_size),
        ..Settings::default()
    };
    for flag in &profile.extra {
        match flag {
            ExtraFlag::Threads(n) => settings.threads = Some(u32::from(*n)),
            ExtraFlag::Parallel(n) => settings.parallel = Some(u32::from(*n)),
            ExtraFlag::NoMmap | ExtraFlag::Mlock => {}
        }
    }
    settings
}

/// What a running process was started with, from its argument vector (without the program).
/// Only the long flags this application emits are read; a flag that is absent or has an
/// unusable value stays unknown. The llama-server API does not report cache types or flash
/// attention, so the process's own command line is the evidence that a change reached it.
pub fn observed_from_arguments(args: &[String]) -> Settings {
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let number = |name: &str| value(name).and_then(|v| v.parse::<u32>().ok());
    let cache = |name: &str| {
        value(name)
            .filter(|v| matches!(*v, "f16" | "q8_0" | "q4_0"))
            .map(str::to_string)
    };
    Settings {
        context_length: number("--ctx-size"),
        gpu_layers: number("--n-gpu-layers"),
        cache_type_k: cache("--cache-type-k"),
        cache_type_v: cache("--cache-type-v"),
        flash_attention: match value("--flash-attn") {
            Some("on") => Some(true),
            Some("off") => Some(false),
            _ => None,
        },
        ubatch_size: number("--ubatch-size"),
        threads: number("--threads"),
        parallel: number("--parallel"),
    }
}

/// Smallest context the launcher accepts (see `LaunchProfile::validate`).
const MIN_CONTEXT: u32 = 256;
/// The server splits its context over this many slots unless `--parallel` says otherwise.
const DEFAULT_SLOTS: u32 = 4;

/// A copy of `profile` with exactly one declared change, or the reason it cannot be made.
/// `baseline_prompt_tokens` is the baseline's measured prompt length; a change that would
/// leave the prompt and answer without room is refused.
pub fn with_variable(
    profile: &LaunchProfile,
    variable: Variable,
    baseline_prompt_tokens: Option<u64>,
    max_tokens: u32,
) -> Result<LaunchProfile, String> {
    let mut changed = profile.clone();
    match variable {
        Variable::CacheTypeQ8 => {
            if profile.cache_type_k == CacheType::Q8_0 {
                return Err("The K cache already uses q8_0.".into());
            }
            changed.cache_type_k = CacheType::Q8_0;
        }
        Variable::FlashAttentionOn => {
            if profile.flash_attention {
                return Err("Flash attention is already on.".into());
            }
            changed.flash_attention = true;
        }
        Variable::HalfContext => {
            let half = profile.context_length / 2;
            if half < MIN_CONTEXT {
                return Err("Half the context is below the smallest the launcher accepts.".into());
            }
            let Some(prompt) = baseline_prompt_tokens else {
                return Err(
                    "The baseline did not report its prompt length, so the change cannot be shown to leave room for it.".into(),
                );
            };
            let slots = requested_settings(profile)
                .parallel
                .unwrap_or(DEFAULT_SLOTS)
                .max(1);
            if prompt + u64::from(max_tokens) > u64::from(half / slots) {
                return Err(
                    "Half the context would not leave room for the prompt and the answer.".into(),
                );
            }
            changed.context_length = half;
        }
    }
    changed.validate()?;
    Ok(changed)
}

#[cfg(test)]
mod tests;
