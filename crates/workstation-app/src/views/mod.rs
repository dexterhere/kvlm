// SPDX-License-Identifier: GPL-3.0-or-later
//! Empty-state views for the development foundation; no fabricated telemetry.

#[cfg(test)]
mod a11y_smoke;
pub mod access;
pub mod attached;
pub mod bench_lab;
pub mod benchmark_history;
pub mod catalog;
pub mod downloads;
pub mod lab;
mod lab_text;
pub mod managed;
mod managed_editor;
pub mod model_fit;
pub mod overview;
pub(super) mod resource_summary;
pub mod runtimes;
pub mod setup;
pub mod updates;
