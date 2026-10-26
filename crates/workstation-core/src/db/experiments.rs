// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark Lab storage: experiments, their passes and diagnostic findings. Call from a
//! blocking worker. Existing benchmark history tables are untouched.

use rusqlite::{OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};

use super::queries::HistoryStore;
use crate::experiments::{
    Engine, Experiment, Finding, FindingStatus, MAX_TEXT, NewExperiment, Ownership, Pass, PassKind,
    PassMetrics, Settings, State, Summary, Variable,
};

/// Stored experiments kept; older finished ones are pruned with their passes and findings.
pub const MAX_EXPERIMENTS: i64 = 200;

fn ms(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Timestamp is out of range.".to_string())
}

fn to_ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn encode<T: Serialize>(value: &T, what: &str) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| format!("Cannot encode the {what}."))
}

fn decode<T: DeserializeOwned>(json: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|_| "A stored experiment is unreadable.".to_string())
}

fn bounded(text: Option<&str>) -> Option<String> {
    text.map(|t| {
        t.chars()
            .filter(|c| !c.is_control())
            .take(MAX_TEXT)
            .collect()
    })
}

/// One line of the experiment list.
#[derive(Clone, Debug, PartialEq)]
pub struct ExperimentRow {
    pub id: i64,
    pub state: State,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
    pub engine: Engine,
    pub ownership: Ownership,
    pub cold: bool,
    pub workload_id: String,
    pub model_name: String,
    pub variable: Option<Variable>,
    pub baseline_id: Option<i64>,
    pub summary: Option<Summary>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub rows: Vec<ExperimentRow>,
    /// Rows that could not be decoded; they are kept in the file, not shown.
    pub unreadable: u32,
}

const EXPERIMENT_COLUMNS: &str = "id,state,started_at_unix_ms,finished_at_unix_ms,engine,ownership,cold,passes_requested,workload_id,prompt_sha256,seed,max_tokens,model_name,model_sha256,runtime_version,backend,hardware_hash,requested_json,observed_json,summary_json,variable,baseline_id,failure";

fn experiment_from(row: &Row) -> rusqlite::Result<Result<Experiment, String>> {
    let id: i64 = row.get(0)?;
    let parsed = (|| -> Result<Experiment, String> {
        let state = State::parse(&row.get::<_, String>(1).map_err(|_| "bad row")?)
            .ok_or("A stored experiment is unreadable.")?;
        let text = |i: usize| {
            row.get::<_, Option<String>>(i)
                .map_err(|_| "bad row".to_string())
        };
        let spec = NewExperiment {
            engine: Engine::parse(&row.get::<_, String>(4).map_err(|_| "bad row")?)
                .ok_or("A stored experiment is unreadable.")?,
            ownership: Ownership::parse(&row.get::<_, String>(5).map_err(|_| "bad row")?)
                .ok_or("A stored experiment is unreadable.")?,
            cold: row.get::<_, i64>(6).map_err(|_| "bad row")? != 0,
            passes_requested: u32::try_from(row.get::<_, i64>(7).map_err(|_| "bad row")?)
                .unwrap_or(0),
            workload_id: row.get(8).map_err(|_| "bad row")?,
            prompt_sha256: row.get(9).map_err(|_| "bad row")?,
            seed: u32::try_from(row.get::<_, i64>(10).map_err(|_| "bad row")?).unwrap_or(0),
            max_tokens: u32::try_from(row.get::<_, i64>(11).map_err(|_| "bad row")?).unwrap_or(0),
            model_name: row.get(12).map_err(|_| "bad row")?,
            model_sha256: text(13)?,
            runtime_version: text(14)?,
            backend: text(15)?,
            hardware_hash: text(16)?,
            requested: decode(&row.get::<_, String>(17).map_err(|_| "bad row")?)?,
            variable: text(20)?.and_then(|v| Variable::parse(&v)),
            baseline_id: row.get(21).map_err(|_| "bad row")?,
        };
        Ok(Experiment {
            id,
            state,
            started_at_unix_ms: to_ms(row.get(2).map_err(|_| "bad row")?),
            finished_at_unix_ms: row
                .get::<_, Option<i64>>(3)
                .map_err(|_| "bad row")?
                .map(to_ms),
            spec,
            observed: text(18)?.map(|j| decode::<Settings>(&j)).transpose()?,
            summary: text(19)?.map(|j| decode::<Summary>(&j)).transpose()?,
            failure: text(22)?,
            passes: Vec::new(),
            findings: Vec::new(),
        })
    })();
    Ok(parsed)
}

impl HistoryStore {
    /// Start an experiment. Only one can run at a time, and the oldest finished ones are
    /// pruned beyond [`MAX_EXPERIMENTS`] in the same transaction.
    pub fn begin_experiment(&self, spec: &NewExperiment, now_ms: u64) -> Result<i64, String> {
        spec.validate()?;
        let requested = encode(&spec.requested, "settings")?;
        let now = ms(now_ms)?;
        self.write(|c| {
            let running: i64 = c
                .query_row(
                    "SELECT count(*) FROM experiments WHERE state='running'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|_| "Cannot read experiments.")?;
            if running > 0 {
                return Err("Another experiment is still running.".into());
            }
            c.execute(
                "DELETE FROM experiments WHERE state<>'running' AND id NOT IN
                 (SELECT id FROM experiments ORDER BY id DESC LIMIT ?1)",
                [MAX_EXPERIMENTS - 1],
            )
            .map_err(|_| "Cannot prune old experiments.")?;
            c.execute(
                "INSERT INTO experiments(state,started_at_unix_ms,engine,ownership,cold,passes_requested,
                    workload_id,prompt_sha256,seed,max_tokens,model_name,model_sha256,runtime_version,
                    backend,hardware_hash,requested_json,variable,baseline_id)
                 VALUES ('running',?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    now,
                    spec.engine.as_str(),
                    spec.ownership.as_str(),
                    i64::from(spec.cold),
                    spec.passes_requested,
                    spec.workload_id,
                    spec.prompt_sha256.to_ascii_lowercase(),
                    spec.seed,
                    spec.max_tokens,
                    spec.model_name,
                    spec.model_sha256,
                    spec.runtime_version,
                    spec.backend,
                    spec.hardware_hash,
                    requested,
                    spec.variable.map(Variable::as_str),
                    spec.baseline_id,
                ],
            )
            .map_err(|_| "Cannot record the experiment. The baseline may no longer exist.")?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Start a pass of a running experiment.
    pub fn begin_pass(
        &self,
        experiment_id: i64,
        kind: PassKind,
        ordinal: u32,
        now_ms: u64,
    ) -> Result<i64, String> {
        let now = ms(now_ms)?;
        self.write(|c| {
            let state: Option<String> = c
                .query_row(
                    "SELECT state FROM experiments WHERE id=?1",
                    [experiment_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| "Cannot read the experiment.")?;
            if state.as_deref() != Some("running") {
                return Err("The experiment is not running.".into());
            }
            c.execute(
                "INSERT INTO experiment_passes(experiment_id,kind,ordinal,state,started_at_unix_ms)
                 VALUES (?1,?2,?3,'running',?4)",
                params![experiment_id, kind.as_str(), ordinal, now],
            )
            .map_err(|_| "Cannot record the pass.")?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Finish a pass. A pass already finished keeps its first result.
    pub fn finish_pass(
        &self,
        pass_id: i64,
        state: State,
        metrics: Option<&PassMetrics>,
        failure: Option<&str>,
        now_ms: u64,
    ) -> Result<(), String> {
        if !state.is_terminal() {
            return Err("A finished pass needs a final state.".into());
        }
        let json = metrics.map(|m| encode(m, "metrics")).transpose()?;
        let now = ms(now_ms)?;
        self.write(|c| {
            let changed = c
                .execute(
                    "UPDATE experiment_passes SET state=?1, finished_at_unix_ms=?2, metrics_json=?3, failure=?4
                     WHERE id=?5 AND state IN ('running','interrupted')",
                    params![state.as_str(), now, json, bounded(failure), pass_id],
                )
                .map_err(|_| "Cannot record the pass result.")?;
            if changed == 0 {
                return Err("That pass is already finished.".into());
            }
            Ok(())
        })
    }

    /// Finish an experiment. One recovered as interrupted can still be completed by the
    /// run that was in fact alive.
    pub fn finish_experiment(
        &self,
        id: i64,
        state: State,
        observed: Option<&Settings>,
        summary: Option<&Summary>,
        failure: Option<&str>,
        now_ms: u64,
    ) -> Result<(), String> {
        if !state.is_terminal() {
            return Err("A finished experiment needs a final state.".into());
        }
        let observed = observed.map(|o| encode(o, "settings")).transpose()?;
        let summary = summary.map(|s| encode(s, "summary")).transpose()?;
        let now = ms(now_ms)?;
        self.write(|c| {
            let changed = c
                .execute(
                    "UPDATE experiments SET state=?1, finished_at_unix_ms=?2, observed_json=?3, summary_json=?4, failure=?5
                     WHERE id=?6 AND state IN ('running','interrupted')",
                    params![state.as_str(), now, observed, summary, bounded(failure), id],
                )
                .map_err(|_| "Cannot record the experiment result.")?;
            if changed == 0 {
                return Err("That experiment is already finished.".into());
            }
            Ok(())
        })
    }

    /// Mark everything still running as interrupted. Call once at start-up, before any new
    /// experiment: a record left running means the application ended mid-run.
    pub fn recover_interrupted(&self, now_ms: u64) -> Result<u64, String> {
        let now = ms(now_ms)?;
        self.write(|c| {
            c.execute(
                "UPDATE experiment_passes SET state='interrupted', finished_at_unix_ms=?1 WHERE state='running'",
                [now],
            )
            .map_err(|_| "Cannot recover interrupted passes.")?;
            let n = c
                .execute(
                    "UPDATE experiments SET state='interrupted', finished_at_unix_ms=?1 WHERE state='running'",
                    [now],
                )
                .map_err(|_| "Cannot recover interrupted experiments.")?;
            Ok(n as u64)
        })
    }

    /// Replace the findings of a finished experiment.
    pub fn save_findings(&self, experiment_id: i64, findings: &[Finding]) -> Result<(), String> {
        let encoded: Vec<(String, &'static str, String)> = findings
            .iter()
            .map(|f| Ok((f.rule_id.clone(), f.status.as_str(), encode(f, "finding")?)))
            .collect::<Result<_, String>>()?;
        self.write(|c| {
            let state: Option<String> = c
                .query_row(
                    "SELECT state FROM experiments WHERE id=?1",
                    [experiment_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| "Cannot read the experiment.")?;
            if state.as_deref().and_then(State::parse).is_none_or(|s| !s.is_terminal()) {
                return Err("Findings can be saved only for a finished experiment.".into());
            }
            c.execute(
                "DELETE FROM diagnostic_findings WHERE experiment_id=?1",
                [experiment_id],
            )
            .map_err(|_| "Cannot replace findings.")?;
            for (rule, status, json) in &encoded {
                c.execute(
                    "INSERT INTO diagnostic_findings(experiment_id,rule_id,status,finding_json) VALUES (?1,?2,?3,?4)",
                    params![experiment_id, rule, status, json],
                )
                .map_err(|_| "Cannot record a finding. Each rule can appear once.")?;
            }
            Ok(())
        })
    }

    /// Newest first. Unreadable rows are counted, not hidden and not fatal.
    pub fn list_experiments(&self, limit: u32) -> Result<Listing, String> {
        let limit = i64::from(limit.clamp(1, MAX_EXPERIMENTS as u32));
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {EXPERIMENT_COLUMNS} FROM experiments ORDER BY id DESC LIMIT ?1"
            ))
            .map_err(|_| "Cannot read experiments.")?;
        let rows = statement
            .query_map([limit], experiment_from)
            .map_err(|_| "Cannot read experiments.")?;
        let mut listing = Listing::default();
        for row in rows {
            match row.map_err(|_| "Cannot read experiments.")? {
                Ok(e) => listing.rows.push(ExperimentRow {
                    id: e.id,
                    state: e.state,
                    started_at_unix_ms: e.started_at_unix_ms,
                    finished_at_unix_ms: e.finished_at_unix_ms,
                    engine: e.spec.engine,
                    ownership: e.spec.ownership,
                    cold: e.spec.cold,
                    workload_id: e.spec.workload_id,
                    model_name: e.spec.model_name,
                    variable: e.spec.variable,
                    baseline_id: e.spec.baseline_id,
                    summary: e.summary,
                }),
                Err(_) => listing.unreadable += 1,
            }
        }
        Ok(listing)
    }

    /// One experiment with its passes and findings.
    pub fn load_experiment(&self, id: i64) -> Result<Option<Experiment>, String> {
        let found = self
            .connection
            .query_row(
                &format!("SELECT {EXPERIMENT_COLUMNS} FROM experiments WHERE id=?1"),
                [id],
                experiment_from,
            )
            .optional()
            .map_err(|_| "Cannot read the experiment.")?;
        let Some(parsed) = found else {
            return Ok(None);
        };
        let mut experiment = parsed?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,kind,ordinal,state,started_at_unix_ms,finished_at_unix_ms,metrics_json,failure
                 FROM experiment_passes WHERE experiment_id=?1 ORDER BY id",
            )
            .map_err(|_| "Cannot read passes.")?;
        let passes = statement
            .query_map([id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|_| "Cannot read passes.")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Cannot read passes.")?;
        for (pass_id, kind, ordinal, state, started, finished, metrics, failure) in passes {
            experiment.passes.push(Pass {
                id: pass_id,
                kind: PassKind::parse(&kind).ok_or("A stored pass is unreadable.")?,
                ordinal: u32::try_from(ordinal).unwrap_or(0),
                state: State::parse(&state).ok_or("A stored pass is unreadable.")?,
                started_at_unix_ms: to_ms(started),
                finished_at_unix_ms: finished.map(to_ms),
                metrics: metrics.map(|j| decode::<PassMetrics>(&j)).transpose()?,
                failure,
            });
        }
        let mut statement = self
            .connection
            .prepare("SELECT status,finding_json FROM diagnostic_findings WHERE experiment_id=?1 ORDER BY rule_id")
            .map_err(|_| "Cannot read findings.")?;
        let findings = statement
            .query_map([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|_| "Cannot read findings.")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Cannot read findings.")?;
        for (status, json) in findings {
            FindingStatus::parse(&status).ok_or("A stored finding is unreadable.")?;
            experiment.findings.push(decode::<Finding>(&json)?);
        }
        Ok(Some(experiment))
    }

    /// Remove a finished experiment with its passes and findings. A follow-up that used it
    /// as a baseline keeps existing, with the link emptied.
    pub fn delete_experiment(&self, id: i64) -> Result<(), String> {
        self.write(|c| {
            let changed = c
                .execute(
                    "DELETE FROM experiments WHERE id=?1 AND state<>'running'",
                    [id],
                )
                .map_err(|_| "Cannot remove the experiment.")?;
            if changed == 0 {
                return Err("Only a finished experiment can be removed.".into());
            }
            Ok(())
        })
    }
}
