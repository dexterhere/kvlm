// SPDX-License-Identifier: GPL-3.0-or-later

use rusqlite::Connection;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use workstation_core::{
    db::{
        HistoryStore,
        profiles::InstallRow,
        schema::{APPLICATION_ID, VERSION},
    },
    runtime::profile::{CacheType, ExtraFlag, LaunchProfile},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/tmp/kvlm-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("w.db")
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn profile() -> LaunchProfile {
    LaunchProfile {
        name: "default".into(),
        model_path: "/models/a.gguf".into(),
        port: 8081,
        context_length: 4096,
        gpu_layers: 99,
        cache_type_k: CacheType::F16,
        cache_type_v: CacheType::F16,
        flash_attention: false,
        ubatch_size: 512,
        extra: vec![ExtraFlag::Threads(8), ExtraFlag::NoMmap],
    }
}

#[test]
fn arguments_are_a_vector_with_owned_flags_once_and_loopback_host() {
    let args = profile().arguments("0123abcd").unwrap();
    for owned in ["--model", "--host", "--port", "--alias"] {
        assert_eq!(args.iter().filter(|a| *a == owned).count(), 1, "{owned}");
    }
    let at = |flag: &str| args.iter().position(|a| a == flag).unwrap();
    assert_eq!(args[at("--host") + 1], "127.0.0.1");
    assert_eq!(args[at("--alias") + 1], "0123abcd");
    assert_eq!(args[at("--model") + 1], "/models/a.gguf");
    assert!(args.windows(2).any(|w| w == ["--threads", "8"]));
    assert!(args.contains(&"--no-mmap".to_string()));
}

#[test]
fn hostile_or_out_of_range_profiles_are_rejected() {
    let bad = |edit: fn(&mut LaunchProfile)| {
        let mut p = profile();
        edit(&mut p);
        assert!(p.validate().is_err());
        assert!(p.arguments("0123abcd").is_err());
    };
    bad(|p| p.name = "a b".into());
    bad(|p| p.name = "x".repeat(65));
    bad(|p| p.model_path = "relative.gguf".into());
    bad(|p| p.model_path = "/m/../etc/x".into());
    bad(|p| p.model_path = "/m/a\nb.gguf".into());
    bad(|p| p.model_path = format!("/{}", "a".repeat(5000)));
    bad(|p| p.port = 80);
    bad(|p| p.context_length = 10);
    bad(|p| p.context_length = 2_000_000);
    bad(|p| p.gpu_layers = 1000);
    bad(|p| p.ubatch_size = 0);
    bad(|p| p.extra = vec![ExtraFlag::Threads(0)]);
    bad(|p| p.extra = vec![ExtraFlag::Mlock, ExtraFlag::Mlock]);
    bad(|p| p.extra = vec![ExtraFlag::Mlock; 9]);
    bad(|p| p.cache_type_v = CacheType::Q8_0);
    assert!(profile().arguments("NOT-HEX!").is_err());
    assert!(profile().arguments("--port").is_err());
}

#[test]
fn unknown_fields_and_free_text_flags_do_not_deserialize() {
    let mut value = serde_json::to_value(profile()).unwrap();
    value["host"] = "0.0.0.0".into();
    assert!(serde_json::from_value::<LaunchProfile>(value).is_err());
    let mut value = serde_json::to_value(profile()).unwrap();
    value["extra"] = serde_json::json!([{ "raw": "--api-key x" }]);
    assert!(serde_json::from_value::<LaunchProfile>(value).is_err());
}

#[test]
fn profiles_round_trip_replace_and_delete() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    store.save_profile(&profile(), 1).unwrap();
    let mut changed = profile();
    changed.port = 9000;
    store.save_profile(&changed, 2).unwrap();
    assert_eq!(store.list_profiles().unwrap(), vec![changed.clone()]);
    assert_eq!(store.get_profile("default").unwrap(), Some(changed));
    assert!(store.get_profile("bad name").is_err());
    let mut invalid = profile();
    invalid.port = 1;
    assert!(store.save_profile(&invalid, 3).is_err());
    store.delete_profile("default").unwrap();
    assert!(store.list_profiles().unwrap().is_empty());
}

#[test]
fn a_tampered_stored_profile_is_skipped_not_trusted() {
    let dir = Dir::new();
    {
        let store = HistoryStore::open(&dir.db()).unwrap();
        store.save_profile(&profile(), 1).unwrap();
    }
    let raw = Connection::open(dir.db()).unwrap();
    raw.execute(
        "UPDATE runtime_profiles SET profile_json=replace(profile_json,'8081','80')",
        [],
    )
    .unwrap();
    drop(raw);
    let store = HistoryStore::open(&dir.db()).unwrap();
    assert!(store.list_profiles().unwrap().is_empty());
    assert!(store.get_profile("default").is_err());
}

#[test]
fn installs_are_recorded_listed_and_removed() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    let row = InstallRow {
        version: "b11905".into(),
        backend: "cpu".into(),
        dir_name: "b11905-cpu".into(),
        sha256: "a".repeat(64),
        installed_at_unix_ms: 5,
    };
    store.record_install(&row).unwrap();
    store.record_install(&row).unwrap();
    assert_eq!(store.list_installs().unwrap(), vec![row.clone()]);
    let mut bad = row.clone();
    bad.dir_name = "../x".into();
    assert!(store.record_install(&bad).is_err());
    bad = row.clone();
    bad.backend = "metal".into();
    assert!(store.record_install(&bad).is_err());
    store.delete_install("b11905", "cpu").unwrap();
    assert!(store.list_installs().unwrap().is_empty());
}

#[test]
fn a_v2_database_upgrades_and_keeps_its_history() {
    let dir = Dir::new();
    // Open once to build the layout, then drop the v3 and v4 tables and step the version back:
    // exactly what an older install holds.
    drop(HistoryStore::open(&dir.db()).unwrap());
    let connection = Connection::open(dir.db()).unwrap();
    connection
        .execute_batch(
            "INSERT INTO benchmark_runs(started_at_unix_ms,snapshot_json) VALUES (7,'{}');
             DROP TABLE diagnostic_findings; DROP TABLE experiment_passes; DROP TABLE experiments;
             DROP TABLE runtime_profiles; DROP TABLE runtime_installs;",
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    let store = HistoryStore::open(&dir.db()).unwrap();
    store.save_profile(&profile(), 1).unwrap();
    drop(store);
    let connection = Connection::open(dir.db()).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    let app: i64 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap();
    let kept: i64 = connection
        .query_row("SELECT started_at_unix_ms FROM benchmark_runs", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!((version, app, kept), (VERSION, APPLICATION_ID, 7));
}
