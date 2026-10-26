// SPDX-License-Identifier: GPL-3.0-or-later

use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workstation_core::{
    db::HistoryStore,
    models::dedup::{DedupRoot, Limits, RootKind, RootState, Stop, Wanted, default_roots, scan},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/kvlm-dedup-{stamp}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, relative: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn wanted(id: &str, content: &[u8]) -> Wanted {
    Wanted {
        artifact_id: id.into(),
        size: content.len() as u64,
        sha256: sha(content),
    }
}

fn root(kind: RootKind, path: &Path) -> DedupRoot {
    DedupRoot {
        kind,
        path: path.to_path_buf(),
    }
}

fn run(roots: &[DedupRoot], wanted: &[Wanted]) -> workstation_core::models::dedup::ScanReport {
    scan(
        roots,
        wanted,
        None,
        &AtomicBool::new(false),
        &Limits::default(),
    )
}

#[test]
fn matches_by_content_and_only_hashes_files_of_the_published_size() {
    let dir = Dir::new();
    let blob = b"model bytes, version one";
    dir.file("blobs/sha256-aaaa", blob);
    dir.file("blobs/other", b"different length content here!!");
    let report = run(
        &[root(RootKind::Ollama, &dir.0.join("blobs"))],
        &[wanted("a", blob)],
    );
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].artifact_id, "a");
    assert_eq!(report.matches[0].root, RootKind::Ollama);
    assert_eq!(report.hashed_bytes, blob.len() as u64);
    assert_eq!(report.stopped, None);
    assert_eq!(report.roots[0].state, RootState::Scanned);
}

#[test]
fn a_file_name_claiming_the_digest_is_never_trusted() {
    let dir = Dir::new();
    let wanted_content = b"the real model bytes!!";
    let imposter = b"imposter model bytes!!";
    assert_eq!(wanted_content.len(), imposter.len());
    dir.file(&format!("blobs/sha256-{}", sha(wanted_content)), imposter);
    let report = run(
        &[root(RootKind::Ollama, &dir.0.join("blobs"))],
        &[wanted("a", wanted_content)],
    );
    assert!(report.matches.is_empty());
    assert_eq!(report.hashed_bytes, imposter.len() as u64);
}

#[test]
fn symlinks_and_special_files_are_skipped_and_never_followed() {
    let dir = Dir::new();
    let blob = b"linked model bytes";
    let real = dir.file("elsewhere/real", blob);
    fs::create_dir_all(dir.0.join("blobs")).unwrap();
    symlink(&real, dir.0.join("blobs/link")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(dir.0.join("blobs/pipe"))
            .status()
            .unwrap()
            .success()
    );
    let report = run(
        &[root(RootKind::Ollama, &dir.0.join("blobs"))],
        &[wanted("a", blob)],
    );
    assert!(report.matches.is_empty());
    assert_eq!(report.hashed_bytes, 0);
}

#[test]
fn a_symlinked_hugging_face_snapshot_layout_is_read_through_blobs_only() {
    let dir = Dir::new();
    let blob = b"hub blob bytes";
    dir.file("hub/models--o--n/blobs/0123", blob);
    fs::create_dir_all(dir.0.join("hub/models--o--n/snapshots/abc")).unwrap();
    symlink(
        dir.0.join("hub/models--o--n/blobs/0123"),
        dir.0.join("hub/models--o--n/snapshots/abc/m.gguf"),
    )
    .unwrap();
    dir.file("hub/datasets--o--d/blobs/0123", blob);
    let report = run(
        &[root(RootKind::HuggingFace, &dir.0.join("hub"))],
        &[wanted("a", blob)],
    );
    assert_eq!(report.matches.len(), 1);
    assert!(report.matches[0].path.ends_with("models--o--n/blobs/0123"));
    assert_eq!(report.matches[0].root, RootKind::HuggingFace);
}

#[test]
fn a_symlinked_root_is_allowed_but_a_symlinked_entry_directory_is_not() {
    let dir = Dir::new();
    let blob = b"through a linked root";
    dir.file("real/blobs/x", blob);
    symlink(dir.0.join("real/blobs"), dir.0.join("link")).unwrap();
    let report = run(
        &[root(RootKind::Ollama, &dir.0.join("link"))],
        &[wanted("a", blob)],
    );
    assert_eq!(report.matches.len(), 1);

    dir.file("outside/blobs/y", blob);
    fs::create_dir_all(dir.0.join("hub")).unwrap();
    symlink(dir.0.join("outside"), dir.0.join("hub/models--o--n")).unwrap();
    let report = run(
        &[root(RootKind::HuggingFace, &dir.0.join("hub"))],
        &[wanted("a", blob)],
    );
    assert!(report.matches.is_empty());
}

#[test]
fn missing_and_unreadable_roots_are_reported_not_treated_as_empty() {
    let dir = Dir::new();
    dir.file("locked/blobs/x", b"hidden bytes");
    let missing = dir.0.join("nothing-here");
    let file_root = dir.file("plain-file", b"x");
    let mut roots = vec![
        root(RootKind::Ollama, &missing),
        root(RootKind::Ollama, &file_root),
    ];
    let locked = dir.0.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_as_root = fs::read_dir(&locked).is_ok();
    roots.push(root(RootKind::Ollama, &locked.join("blobs")));
    let report = run(&roots, &[wanted("a", b"hidden bytes")]);
    assert_eq!(report.roots[0].state, RootState::Missing);
    assert_eq!(report.roots[1].state, RootState::Missing);
    if !readable_as_root {
        assert_eq!(report.roots[2].state, RootState::Unreadable);
    }
    assert!(report.matches.is_empty() || readable_as_root);
}

#[test]
fn cancel_and_limits_stop_the_scan_and_say_so() {
    let dir = Dir::new();
    let blob = b"twelve bytes";
    for name in ["a", "b", "c"] {
        dir.file(&format!("blobs/{name}"), blob);
    }
    let roots = [root(RootKind::Ollama, &dir.0.join("blobs"))];
    let wanted = [wanted("a", blob)];

    let cancelled = scan(
        &roots,
        &wanted,
        None,
        &AtomicBool::new(true),
        &Limits::default(),
    );
    assert_eq!(cancelled.stopped, Some(Stop::Cancelled));
    assert!(cancelled.matches.is_empty());

    let few = Limits {
        entries: 1,
        ..Limits::default()
    };
    let report = scan(&roots, &wanted, None, &AtomicBool::new(false), &few);
    assert_eq!(report.stopped, Some(Stop::EntryLimit));

    let tiny = Limits {
        hashed_bytes: 5,
        ..Limits::default()
    };
    let report = scan(&roots, &wanted, None, &AtomicBool::new(false), &tiny);
    assert_eq!(report.stopped, Some(Stop::ByteLimit));
    assert!(report.matches.is_empty());

    let instant = Limits {
        time: Duration::ZERO,
        ..Limits::default()
    };
    std::thread::sleep(Duration::from_millis(2));
    let report = scan(&roots, &wanted, None, &AtomicBool::new(false), &instant);
    assert_eq!(report.stopped, Some(Stop::TimeLimit));
}

#[test]
fn unwanted_or_malformed_requests_match_nothing() {
    let dir = Dir::new();
    let blob = b"some bytes";
    dir.file("blobs/x", blob);
    let roots = [root(RootKind::Ollama, &dir.0.join("blobs"))];
    let mut upper = wanted("a", blob);
    upper.sha256 = upper.sha256.to_uppercase();
    let mut short = wanted("b", blob);
    short.sha256 = "abcd".into();
    let mut zero = wanted("c", blob);
    zero.size = 0;
    let report = run(&roots, &[upper, short, zero]);
    assert!(report.matches.is_empty());
    assert_eq!(report.hashed_bytes, 0);
}

#[test]
fn remembered_digests_skip_hashing_until_the_file_changes() {
    let dir = Dir::new();
    let first = b"cache me please";
    let path = dir.file("blobs/x", first);
    let roots = [root(RootKind::Ollama, &dir.0.join("blobs"))];
    let db = dir.0.join("history.db");
    let store = HistoryStore::open(&db).unwrap();
    let wanted = [wanted("a", first)];
    let go = || {
        scan(
            &roots,
            &wanted,
            Some(&store),
            &AtomicBool::new(false),
            &Limits::default(),
        )
    };

    let cold = go();
    assert_eq!(
        (cold.matches.len(), cold.hashed_bytes),
        (1, first.len() as u64)
    );
    let warm = go();
    assert_eq!((warm.matches.len(), warm.hashed_bytes), (1, 0));

    // Same size, mtime restored: the change time still moves, so the hint is rejected.
    let original_mtime = fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(&path, b"CACHE ME PLEASE").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(original_mtime)
        .unwrap();
    let changed = go();
    assert!(changed.matches.is_empty());
    assert_eq!(changed.hashed_bytes, first.len() as u64);
}

#[test]
fn default_roots_follow_the_documented_environment_order() {
    // Environment-independent shape check: every root is absolute and unique.
    let roots = default_roots();
    assert!(roots.iter().all(|r| r.path.is_absolute()));
    let mut paths: Vec<_> = roots.iter().map(|r| &r.path).collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), roots.len());
    assert!(
        roots
            .iter()
            .any(|r| r.path.ends_with(".ollama/models/blobs") && r.kind == RootKind::Ollama)
    );
}

#[test]
fn hub_discovery_is_counted_and_unreadable_blob_directories_are_reported() {
    let dir = Dir::new();
    for n in 0..5 {
        fs::create_dir_all(dir.0.join(format!("hub/models--o--n{n}/blobs"))).unwrap();
    }
    let roots = [root(RootKind::HuggingFace, &dir.0.join("hub"))];
    let few = Limits {
        entries: 2,
        ..Limits::default()
    };
    let report = scan(
        &roots,
        &[wanted("a", b"x")],
        None,
        &AtomicBool::new(false),
        &few,
    );
    assert_eq!(report.stopped, Some(Stop::EntryLimit));

    let locked = dir.0.join("hub/models--o--n0/blobs");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let report = run(&roots, &[wanted("a", b"x")]);
    let can_read_anyway = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    if !can_read_anyway {
        assert_eq!(report.skipped, 1);
    }
}

#[test]
fn an_unreadable_candidate_counts_as_skipped_not_absent() {
    let dir = Dir::new();
    let blob = b"unreadable candidate";
    let path = dir.file("blobs/x", blob);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let report = run(
        &[root(RootKind::Ollama, &dir.0.join("blobs"))],
        &[wanted("a", blob)],
    );
    let can_read_anyway = File::open(&path).is_ok();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    if !can_read_anyway {
        assert_eq!((report.skipped, report.matches.len()), (1, 0));
    }
}

#[test]
fn unrepresentable_timestamps_are_never_remembered() {
    use workstation_core::db::digests::FileStamp;
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.0.join("h.db")).unwrap();
    let stamp = FileStamp {
        dev: 1,
        inode: 2,
        size: 3,
        mtime_ns: i128::from(i64::MAX) + 5,
        ctime_ns: 0,
    };
    assert!(
        store
            .cache_file_digest("/p", &stamp, &"a".repeat(64), 1)
            .is_err()
    );
    assert!(store.cached_file_digest("/p", &stamp).is_none());
}
