// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime integrations. Attached Ollama discovery and inspection are read-only; process control is planned.

pub mod catalog_service;
pub mod downloader;
pub mod huggingface;
pub mod llamacpp;
pub mod lmstudio;
pub mod ollama;
pub mod supervisor;
#[cfg(test)]
mod test_server;
pub mod traits;
pub mod vllm;
