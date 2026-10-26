// SPDX-License-Identifier: GPL-3.0-or-later
//! GTK-independent domain and services. Planned modules contain no operational stubs.

pub mod db;
pub mod experiments;
pub mod export;
pub mod fit;
pub mod hardware;
pub mod models;
pub mod runtime;

/// Provisional application identifier from the master specification.
pub const APP_ID: &str = "org.gnome.LocalAIWorkstation";
pub const APP_NAME: &str = "Local AI Workstation";
