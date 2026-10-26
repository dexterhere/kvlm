// SPDX-License-Identifier: GPL-3.0-or-later
//! Persisted launch profiles and installed runtime builds. Call from a blocking worker.

use super::queries::HistoryStore;
use crate::runtime::profile::{LaunchProfile, valid_name};
use rusqlite::{OptionalExtension, params};

const MAX_PROFILES: i64 = 100;
const MAX_INSTALLS: i64 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallRow {
    pub version: String,
    pub backend: String,
    pub dir_name: String,
    pub sha256: String,
    pub installed_at_unix_ms: u64,
}

fn ms(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Timestamp is out of range.".to_string())
}

impl HistoryStore {
    /// Insert or replace the profile with this name after validating it.
    pub fn save_profile(&self, profile: &LaunchProfile, now_ms: u64) -> Result<(), String> {
        profile.validate()?;
        let json = serde_json::to_string(profile).map_err(|_| "Cannot encode the profile.")?;
        let now = ms(now_ms)?;
        self.write(|c| {
            let known: i64 = c
                .query_row(
                    "SELECT count(*) FROM runtime_profiles WHERE name<>?1",
                    [&profile.name],
                    |r| r.get(0),
                )
                .map_err(|_| "Cannot read profiles.")?;
            if known >= MAX_PROFILES {
                return Err("Too many profiles. Remove one first.".into());
            }
            c.execute(
                "INSERT INTO runtime_profiles(name,engine,profile_json,created_at_unix_ms,updated_at_unix_ms)
                 VALUES (?1,'llamacpp',?2,?3,?3)
                 ON CONFLICT(name) DO UPDATE SET profile_json=excluded.profile_json, updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![profile.name, json, now],
            )
            .map_err(|_| "Cannot save the profile.")?;
            Ok(())
        })
    }

    pub fn get_profile(&self, name: &str) -> Result<Option<LaunchProfile>, String> {
        if !valid_name(name) {
            return Err("Profile name is invalid.".into());
        }
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT profile_json FROM runtime_profiles WHERE name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Cannot read profiles.")?;
        json.map(|j| decode_profile(&j)).transpose()
    }

    /// Profiles whose stored JSON no longer validates are skipped, never trusted.
    pub fn list_profiles(&self) -> Result<Vec<LaunchProfile>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT profile_json FROM runtime_profiles ORDER BY name LIMIT 100")
            .map_err(|_| "Cannot read profiles.")?;
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| "Cannot read profiles.")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Cannot read profiles.")?;
        Ok(rows.iter().filter_map(|j| decode_profile(j).ok()).collect())
    }

    pub fn delete_profile(&self, name: &str) -> Result<(), String> {
        self.write(|c| {
            c.execute("DELETE FROM runtime_profiles WHERE name=?1", [name])
                .map_err(|_| "Cannot remove the profile.")?;
            Ok(())
        })
    }

    /// Record a runtime build this application published. Same version and backend replaces.
    pub fn record_install(&self, row: &InstallRow) -> Result<(), String> {
        if !valid_name(&row.dir_name)
            || !valid_name(&row.version)
            || !matches!(row.backend.as_str(), "cpu" | "vulkan" | "cuda")
            || row.sha256.len() != 64
        {
            return Err("Install record is invalid.".into());
        }
        let at = ms(row.installed_at_unix_ms)?;
        self.write(|c| {
            let count: i64 = c
                .query_row("SELECT count(*) FROM runtime_installs", [], |r| r.get(0))
                .map_err(|_| "Cannot read installs.")?;
            if count >= MAX_INSTALLS {
                return Err("Too many installed builds. Remove one first.".into());
            }
            c.execute(
                "INSERT INTO runtime_installs(engine,version,backend,dir_name,sha256,installed_at_unix_ms)
                 VALUES ('llamacpp',?1,?2,?3,?4,?5)
                 ON CONFLICT(engine,version,backend) DO UPDATE SET dir_name=excluded.dir_name, sha256=excluded.sha256, installed_at_unix_ms=excluded.installed_at_unix_ms",
                params![row.version, row.backend, row.dir_name, row.sha256, at],
            )
            .map_err(|_| "Cannot record the install.")?;
            Ok(())
        })
    }

    pub fn list_installs(&self) -> Result<Vec<InstallRow>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT version,backend,dir_name,sha256,installed_at_unix_ms FROM runtime_installs ORDER BY installed_at_unix_ms DESC LIMIT 50")
            .map_err(|_| "Cannot read installs.")?;
        let rows = statement
            .query_map([], |r| {
                Ok(InstallRow {
                    version: r.get(0)?,
                    backend: r.get(1)?,
                    dir_name: r.get(2)?,
                    sha256: r.get(3)?,
                    installed_at_unix_ms: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                })
            })
            .map_err(|_| "Cannot read installs.")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Cannot read installs.")?;
        Ok(rows)
    }

    pub fn delete_install(&self, version: &str, backend: &str) -> Result<(), String> {
        self.write(|c| {
            c.execute(
                "DELETE FROM runtime_installs WHERE engine='llamacpp' AND version=?1 AND backend=?2",
                [version, backend],
            )
            .map_err(|_| "Cannot remove the install record.")?;
            Ok(())
        })
    }
}

fn decode_profile(json: &str) -> Result<LaunchProfile, String> {
    let profile: LaunchProfile =
        serde_json::from_str(json).map_err(|_| "A stored profile is unreadable.")?;
    profile.validate()?;
    Ok(profile)
}
