// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime integrations. Attached Ollama discovery is read-only; process control is planned.

pub mod llamacpp;
pub mod lmstudio;
pub mod ollama;
pub mod supervisor;
pub mod traits;
pub mod vllm;
