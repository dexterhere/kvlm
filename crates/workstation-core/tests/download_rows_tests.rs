// SPDX-License-Identifier: GPL-3.0-or-later

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use workstation_core::{
    db::{
        HistoryStore,
        downloads::{DownloadState, NewDownload},
    },
    models::download::{Storage, finish_hex},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const REV: &str = "0123456789abcdef0123456789abcdef01234567";

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/kvlm-rows-{stamp}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha(content: &[u8]) -> String {
    finish_hex(Sha256::new().chain_update(content))
}

fn request(content: &[u8], name: &str) -> NewDownload {
    NewDownload {
        artifact_id: format!("hf:o/n@{REV}:{name}"),
        repo_id: "o/n".into(),
        revision: REV.into(),
        filename: name.into(),
        sha256: sha(content),
        size: content.len() as u64,
    }
}

fn store(dir: &Dir) -> HistoryStore {
    HistoryStore::open(&dir.0.join("history.db")).unwrap()
}

#[test]
fn a_download_row_is_reused_on_resume_and_only_one_may_be_active() {
    let dir = Dir::new();
    let db = store(&dir);
    let first = db.begin_download(&request(b"aaaa", "a.gguf"), 1).unwrap();
    assert!(
        db.begin_download(&request(b"bbbb", "b.gguf"), 2)
            .unwrap_err()
            .contains("already running")
    );
    db.update_download(first, DownloadState::Paused, 2, None, 3)
        .unwrap();
    let again = db.begin_download(&request(b"aaaa", "a.gguf"), 4).unwrap();
    assert_eq!(again, first);
    let row = db.get_download(first).unwrap().unwrap();
    assert_eq!(
        (row.state, row.bytes_done, row.expected_size),
        (DownloadState::Running, 2, 4)
    );
    db.update_download(first, DownloadState::Failed, 2, Some("network"), 5)
        .unwrap();
    assert_eq!(
        db.get_download(first)
            .unwrap()
            .unwrap()
            .failure_code
            .as_deref(),
        Some("network")
    );
    let other = db.begin_download(&request(b"bbbb", "b.gguf"), 6).unwrap();
    assert_ne!(other, first);
    assert_eq!(db.list_downloads().unwrap().len(), 2);
    db.delete_download(other).unwrap();
    assert!(db.get_download(other).unwrap().is_none());
}

#[test]
fn invalid_requests_are_refused() {
    let dir = Dir::new();
    let db = store(&dir);
    let good = request(b"aaaa", "a.gguf");
    for mutate in [
        |r: &mut NewDownload| r.repo_id = "bad".into(),
        |r: &mut NewDownload| r.revision = "main".into(),
        |r: &mut NewDownload| r.filename = "../x.gguf".into(),
        |r: &mut NewDownload| r.sha256 = "ABC".into(),
        |r: &mut NewDownload| r.size = 0,
    ] {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert!(db.begin_download(&bad, 1).is_err());
    }
    assert!(db.list_downloads().unwrap().is_empty());
}

#[test]
fn reconcile_pauses_running_rows_completes_published_files_and_flags_lost_partials() {
    let dir = Dir::new();
    let db = store(&dir);
    let storage = Storage::open(&dir.0.join("models")).unwrap();

    // Running row with a partial file: becomes paused at the file's real length.
    let content = b"0123456789";
    let running = db.begin_download(&request(content, "run.gguf"), 1).unwrap();
    let (mut part, _) = storage.open_part(&sha(content), 10).unwrap();
    part.write_all(&content[..4]).unwrap();
    db.update_download(running, DownloadState::Running, 9, None, 2)
        .unwrap();

    // Row whose verified file is already published, recorded as still running.
    let done_content = b"already published!";
    db.update_download(running, DownloadState::Paused, 4, None, 2)
        .unwrap();
    let published = db
        .begin_download(&request(done_content, "done.gguf"), 3)
        .unwrap();
    let (mut file, _) = storage.open_part(&sha(done_content), 18).unwrap();
    file.write_all(done_content).unwrap();
    storage
        .publish(
            &sha(done_content),
            file,
            Sha256::new().chain_update(done_content),
        )
        .unwrap();
    db.update_download(published, DownloadState::Verifying, 18, None, 4)
        .unwrap();

    // Paused row that claims bytes but lost its partial file.
    let lost_content = b"lost partial";
    db.update_download(published, DownloadState::Paused, 18, None, 4)
        .unwrap();
    let lost = db
        .begin_download(&request(lost_content, "lost.gguf"), 5)
        .unwrap();
    db.update_download(lost, DownloadState::Running, 5, None, 6)
        .unwrap();

    db.reconcile_downloads(&storage, 100).unwrap();
    let rows: std::collections::HashMap<_, _> = db
        .list_downloads()
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
    assert_eq!(
        (rows[&running].state, rows[&running].bytes_done),
        (DownloadState::Paused, 4)
    );
    assert_eq!(rows[&published].state, DownloadState::Completed);
    assert_eq!(rows[&lost].state, DownloadState::Failed);
    assert_eq!(rows[&lost].failure_code.as_deref(), Some("missing_partial"));
}

#[test]
fn a_fresh_row_that_never_wrote_anything_is_paused_not_failed() {
    let dir = Dir::new();
    let db = store(&dir);
    let storage = Storage::open(&dir.0.join("models")).unwrap();
    let id = db
        .begin_download(&request(b"never started", "n.gguf"), 1)
        .unwrap();
    db.reconcile_downloads(&storage, 2).unwrap();
    let row = db.get_download(id).unwrap().unwrap();
    assert_eq!((row.state, row.failure_code), (DownloadState::Paused, None));
}
