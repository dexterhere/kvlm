// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime ownership and lifecycle vocabulary, and the managed llama.cpp launch profile.
//! Pure data and validation: nothing here starts a process.

pub mod profile;

use serde::{Deserialize, Serialize};

/// Who owns the process. Only `Managed` runtimes may be stopped by this application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnershipMode {
    Attached,
    Managed,
}

impl OwnershipMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Attached => "Attached, externally owned",
            Self::Managed => "Managed by this application",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeLifecycleState {
    NotInstalled,
    Detected,
    Starting,
    Ready,
    Busy,
    Stopping,
    Stopped,
    Failed,
}

impl RuntimeLifecycleState {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "Not installed",
            Self::Detected => "Detected",
            Self::Starting => "Starting",
            Self::Ready => "Ready",
            Self::Busy => "Busy",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }
    /// A process may exist that the supervisor must still account for.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Ready | Self::Busy | Self::Stopping
        )
    }
}
