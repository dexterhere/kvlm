// SPDX-License-Identifier: GPL-3.0-or-later
use std::{env, path::Path, process::ExitCode};

use workstation_core::{
    fit::{
        gguf_fit::{LayoutAssumption, RuntimeScenario, assess_gguf_fit},
        memory_math::{ExecutionTarget, KvCacheType, RuntimeConfig},
    },
    models::gguf::inspect_file,
};

const USAGE: &str = "Usage: gguf_fit <GGUF_PATH> <context_tokens> <microbatch_tokens> <key_cache:f16|q8_0|q4_0> <value_cache:f16|q8_0|q4_0> <sequences:integer|unknown> <available_ram_bytes:integer|unknown> [--assume-uniform-attention]";

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(path) = args.next() else {
        return usage();
    };
    let rest: Vec<_> = args.collect();
    let assume_layout = match rest.len() {
        6 => false,
        7 if rest[6] == "--assume-uniform-attention" => true,
        _ => return usage(),
    };

    let Some(context_tokens) = number(&rest[0]) else {
        return invalid("context_tokens must be a positive integer");
    };
    let Some(micro_batch_tokens) = number(&rest[1]) else {
        return invalid("microbatch_tokens must be a positive integer");
    };
    if context_tokens == 0 || micro_batch_tokens == 0 {
        return invalid("context_tokens and microbatch_tokens must be positive");
    }
    let Some(key_cache) = cache_type(&rest[2]) else {
        return invalid("unknown key cache type");
    };
    let Some(value_cache) = cache_type(&rest[3]) else {
        return invalid("unknown value cache type");
    };
    let Some(sequence_count) = optional_u32(&rest[4]) else {
        return invalid("sequences must be a positive integer or 'unknown'");
    };
    if sequence_count == Some(0) {
        return invalid("sequences must be positive or 'unknown'");
    }
    let Some(available_ram_bytes) = optional_u64(&rest[5]) else {
        return invalid("available_ram_bytes must be an integer or 'unknown'");
    };

    let metadata = match inspect_file(Path::new(&path)) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("GGUF inspection failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let scenario = RuntimeScenario {
        config: RuntimeConfig {
            context_tokens,
            micro_batch_tokens,
            key_cache,
            value_cache,
        },
        sequence_count,
    };
    let target = ExecutionTarget::Cpu {
        available_ram_bytes,
    };
    let layout = if assume_layout {
        LayoutAssumption::UniformFullAttention
    } else {
        LayoutAssumption::Unconfirmed
    };
    let result = match assess_gguf_fit(&metadata, &scenario, &target, layout) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("Fit input rejected: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("GGUF file bytes (size estimate): {}", metadata.file_bytes);
    println!("Declared architecture: {:?}", metadata.architecture);
    println!("Declared LLaMA metadata: {:?}", metadata.llama);
    println!("Layout warnings: {:?}", metadata.warnings);
    println!("Scenario values are user-supplied, not observed runtime settings.");
    println!("Requested context tokens: {context_tokens}");
    println!("Prompt microbatch tokens: {micro_batch_tokens}");
    println!("Key cache: {key_cache:?}; value cache: {value_cache:?}");
    println!("Sequence count: {sequence_count:?}");
    println!("Available RAM bytes: {available_ram_bytes:?}");
    println!("Layout assumption: {layout:?}");
    println!("Runtime compatibility: unverified");
    println!("Status: {:?}", result.status);
    println!("Assessment: {:?}", result.assessment);
    println!("A static estimate is not runtime or benchmark verification.");
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::FAILURE
}

fn invalid(message: &str) -> ExitCode {
    eprintln!("Invalid argument: {message}\n{USAGE}");
    ExitCode::FAILURE
}

fn number(value: &std::ffi::OsStr) -> Option<u64> {
    value.to_str()?.parse().ok()
}

fn optional_u32(value: &std::ffi::OsStr) -> Option<Option<u32>> {
    if value == "unknown" {
        Some(None)
    } else {
        value.to_str()?.parse().ok().map(Some)
    }
}

fn optional_u64(value: &std::ffi::OsStr) -> Option<Option<u64>> {
    if value == "unknown" {
        Some(None)
    } else {
        value.to_str()?.parse().ok().map(Some)
    }
}

fn cache_type(value: &std::ffi::OsStr) -> Option<KvCacheType> {
    match value.to_str()? {
        "f16" => Some(KvCacheType::F16),
        "q8_0" => Some(KvCacheType::Q8_0),
        "q4_0" => Some(KvCacheType::Q4_0),
        _ => None,
    }
}
