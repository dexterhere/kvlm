// SPDX-License-Identifier: GPL-3.0-or-later
//! Managed portable llama.cpp: release selection, verified installation and safe extraction.
//! Process supervision lives in `supervisor`; the HTTP adapter arrives in a later phase.

pub mod extract;
pub mod install;
pub mod release;

#[cfg(test)]
mod extract_tests;
#[cfg(test)]
mod release_tests;
