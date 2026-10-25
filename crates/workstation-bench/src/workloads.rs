// SPDX-License-Identifier: GPL-3.0-or-later
//! One bounded workload; settings describe requested values, not observed allocation.

use workstation_runtimes::ollama::GenerationRequest;

pub const QUICK_WORKLOAD_ID: &str = "quick-text-v1";
pub const QUICK_PROMPT: &str = "Write a short Rust function that sums a slice of u32 values into a u64. Explain why the return type avoids overflow for small inputs. Do not use external crates.";

pub fn quick_request() -> GenerationRequest<'static> {
    GenerationRequest {
        prompt: QUICK_PROMPT,
        num_predict: 128,
        num_ctx: 2048,
        seed: 42,
    }
}
