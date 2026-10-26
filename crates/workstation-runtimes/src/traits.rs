// SPDX-License-Identifier: GPL-3.0-or-later
//! What each engine can do under each ownership mode. Operations outside the table fail
//! with a typed `Unsupported`; nothing is a silent no-op, and an Attached runtime can never
//! be asked to start or stop.

use std::fmt;
use workstation_core::runtime::OwnershipMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Ollama,
    LlamaCpp,
    LmStudio,
    Vllm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Probe,
    ListModels,
    LoadedModels,
    /// Start, stop and restart a process this application owns.
    StartStop,
    /// Ask the runtime to load or unload a model through its own API.
    LoadUnload,
    StreamInference,
}

/// Capabilities implemented today. Planned engines support nothing until their phase lands.
pub fn supports(engine: Engine, ownership: OwnershipMode, capability: Capability) -> bool {
    use Capability::*;
    match (engine, ownership) {
        (Engine::Ollama, OwnershipMode::Attached) => {
            matches!(
                capability,
                Probe | ListModels | LoadedModels | StreamInference
            )
        }
        (Engine::LlamaCpp, OwnershipMode::Managed) => matches!(
            capability,
            Probe | ListModels | LoadedModels | StartStop | StreamInference
        ),
        // Read-only discovery of externally owned servers. They stay unsupported for streaming
        // inference until a benchmark can tell the user that local execution is unproven.
        (Engine::LmStudio | Engine::Vllm, OwnershipMode::Attached) => {
            matches!(capability, Probe | ListModels | LoadedModels)
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub engine: Engine,
    pub ownership: OwnershipMode,
    pub capability: Capability,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} ({}) does not support {:?}.",
            self.engine,
            self.ownership.label(),
            self.capability
        )
    }
}

impl std::error::Error for Unsupported {}

pub fn require(
    engine: Engine,
    ownership: OwnershipMode,
    capability: Capability,
) -> Result<(), Unsupported> {
    if supports(engine, ownership, capability) {
        Ok(())
    } else {
        Err(Unsupported {
            engine,
            ownership,
            capability,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_runtimes_can_never_start_stop_or_load() {
        for engine in [
            Engine::Ollama,
            Engine::LlamaCpp,
            Engine::LmStudio,
            Engine::Vllm,
        ] {
            for capability in [Capability::StartStop, Capability::LoadUnload] {
                assert!(!supports(engine, OwnershipMode::Attached, capability));
            }
        }
    }

    #[test]
    fn only_managed_llamacpp_starts_and_stops() {
        assert!(
            require(
                Engine::LlamaCpp,
                OwnershipMode::Managed,
                Capability::StartStop
            )
            .is_ok()
        );
        let error = require(
            Engine::Ollama,
            OwnershipMode::Managed,
            Capability::StartStop,
        );
        assert_eq!(error.unwrap_err().engine, Engine::Ollama);
        assert!(!supports(
            Engine::Vllm,
            OwnershipMode::Managed,
            Capability::Probe
        ));
        for engine in [Engine::LmStudio, Engine::Vllm] {
            assert!(supports(
                engine,
                OwnershipMode::Attached,
                Capability::ListModels
            ));
            assert!(!supports(
                engine,
                OwnershipMode::Attached,
                Capability::StreamInference
            ));
        }
    }
}
