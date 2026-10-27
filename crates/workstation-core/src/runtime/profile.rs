// SPDX-License-Identifier: GPL-3.0-or-later
//! The managed llama.cpp launch profile. There is no host field (always `127.0.0.1`) and no
//! free-text argument field: every flag comes from a typed field or the closed `ExtraFlag` set,
//! and the flags the supervisor owns are always emitted by the supervisor, exactly once.

use serde::{Deserialize, Serialize};

pub const LOOPBACK: &str = "127.0.0.1";
const MAX_PATH: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheType {
    F16,
    Q8_0,
    Q4_0,
}

impl CacheType {
    pub fn flag_value(self) -> &'static str {
        match self {
            Self::F16 => "f16",
            Self::Q8_0 => "q8_0",
            Self::Q4_0 => "q4_0",
        }
    }
}

/// The only optional flags a profile may carry. Values are typed, never text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtraFlag {
    Threads(u16),
    Parallel(u8),
    NoMmap,
    Mlock,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchProfile {
    pub name: String,
    /// Absolute path of a GGUF file. Existence and ownership are checked at start.
    pub model_path: String,
    pub port: u16,
    pub context_length: u32,
    pub gpu_layers: u16,
    pub cache_type_k: CacheType,
    pub cache_type_v: CacheType,
    pub flash_attention: bool,
    pub ubatch_size: u32,
    pub extra: Vec<ExtraFlag>,
}

pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A launch generation id: lowercase hex, 8 to 64 characters.
pub fn valid_alias(alias: &str) -> bool {
    (8..=64).contains(&alias.len())
        && alias
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl LaunchProfile {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_name(&self.name) {
            return Err("Profile name must be 1 to 64 letters, digits, '.', '_' or '-'.".into());
        }
        let path = &self.model_path;
        if path.is_empty()
            || path.len() > MAX_PATH
            || !path.starts_with('/')
            || path.chars().any(char::is_control)
            || path.split('/').any(|part| part == "..")
        {
            return Err(
                "Model path must be an absolute path without '..' or control characters.".into(),
            );
        }
        if self.port < 1024 {
            return Err("Port must be 1024 or higher.".into());
        }
        if !(256..=1_048_576).contains(&self.context_length) {
            return Err("Context length must be between 256 and 1,048,576.".into());
        }
        if self.gpu_layers > 999 {
            return Err("GPU layers must be 999 or fewer.".into());
        }
        if !(1..=8192).contains(&self.ubatch_size) {
            return Err("Micro-batch size must be between 1 and 8192.".into());
        }
        if self.extra.len() > 8 {
            return Err("Too many optional flags.".into());
        }
        for (index, flag) in self.extra.iter().enumerate() {
            let ok = match flag {
                ExtraFlag::Threads(n) => (1..=1024).contains(n),
                ExtraFlag::Parallel(n) => (1..=64).contains(n),
                ExtraFlag::NoMmap | ExtraFlag::Mlock => true,
            };
            if !ok {
                return Err("An optional flag value is out of range.".into());
            }
            if self.extra[..index]
                .iter()
                .any(|seen| std::mem::discriminant(seen) == std::mem::discriminant(flag))
            {
                return Err("An optional flag is set more than once.".into());
            }
        }
        // A quantized V cache needs flash attention in llama.cpp; refuse the combination
        // here rather than let the server fail at start.
        if self.cache_type_v != CacheType::F16 && !self.flash_attention {
            return Err("A quantized V cache requires flash attention.".into());
        }
        Ok(())
    }

    /// The argument vector for `llama-server` (without the program). `alias` is the random
    /// launch generation id chosen by the supervisor. Validates first; nothing is quoted
    /// because nothing is ever passed through a shell.
    pub fn arguments(&self, alias: &str) -> Result<Vec<String>, String> {
        self.validate()?;
        if !valid_alias(alias) {
            return Err("Launch alias is invalid.".into());
        }
        let mut args: Vec<String> = [
            "--model",
            &self.model_path,
            "--host",
            LOOPBACK,
            "--port",
            &self.port.to_string(),
            "--alias",
            alias,
            "--ctx-size",
            &self.context_length.to_string(),
            "--n-gpu-layers",
            &self.gpu_layers.to_string(),
            "--cache-type-k",
            self.cache_type_k.flag_value(),
            "--cache-type-v",
            self.cache_type_v.flag_value(),
            "--flash-attn",
            if self.flash_attention { "on" } else { "off" },
            "--ubatch-size",
            &self.ubatch_size.to_string(),
            "--metrics",
        ]
        .map(String::from)
        .into();
        for flag in &self.extra {
            match flag {
                ExtraFlag::Threads(n) => args.extend(["--threads".into(), n.to_string()]),
                ExtraFlag::Parallel(n) => args.extend(["--parallel".into(), n.to_string()]),
                ExtraFlag::NoMmap => args.push("--no-mmap".into()),
                ExtraFlag::Mlock => args.push("--mlock".into()),
            }
        }
        Ok(args)
    }
}
