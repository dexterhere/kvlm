// SPDX-License-Identifier: GPL-3.0-or-later
use std::{env, path::Path, process::ExitCode};

use workstation_core::models::gguf::inspect_file;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("Usage: inspect_gguf <GGUF_PATH>");
        return ExitCode::FAILURE;
    };
    if args.next().is_some() {
        eprintln!("Usage: inspect_gguf <GGUF_PATH>");
        return ExitCode::FAILURE;
    }

    let metadata = match inspect_file(Path::new(&path)) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("GGUF inspection failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("File bytes: {}", metadata.file_bytes);
    println!("Tensor count: {}", metadata.tensor_count);
    println!("Tensor bytes: {}", metadata.tensor_bytes);
    println!("Alignment: {}", metadata.alignment);
    println!("Architecture: {:?}", metadata.architecture);
    println!("LLaMA declared metadata: {:?}", metadata.llama);
    println!("Layout warnings: {:?}", metadata.warnings);
    println!("Declared metadata only; runtime compatibility and fit remain unverified.");
    ExitCode::SUCCESS
}
