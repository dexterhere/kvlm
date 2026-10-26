// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::test_server::*;
use std::{
    fs,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use workstation_core::{
    db::downloads::DownloadState,
    models::{
        artifact::{ArtifactFormat, ShardInfo},
        download::finish_hex,
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const REV: &str = "0123456789abcdef0123456789abcdef01234567";
const FILE_LINE: &str =
    "GET /o/n/resolve/0123456789abcdef0123456789abcdef01234567/dir/m.gguf HTTP/1.1";

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/kvlm-dl-{stamp}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn database(&self) -> PathBuf {
        self.0.join("history.db")
    }
    fn models(&self) -> PathBuf {
        self.0.join("models")
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn sha(bytes: &[u8]) -> String {
    finish_hex(Sha256::new().chain_update(bytes))
}

fn artifact(bytes: &[u8]) -> ModelArtifact {
    ModelArtifact {
        artifact_id: format!("hf:o/n@{REV}:dir/m.gguf"),
        repo_id: "o/n".into(),
        revision: REV.into(),
        filename: "dir/m.gguf".into(),
        format: ArtifactFormat::Gguf,
        quantization_hint: None,
        size_bytes: Some(bytes.len() as u64),
        sha256: Some(sha(bytes)),
        shards: None,
    }
}

fn downloader(server: &Server, dir: &Dir) -> Downloader {
    Downloader::with_parts(Arc::new(server.client()), dir.database(), dir.models())
}

async fn run(dl: &Downloader, artifact: &ModelArtifact) -> (DownloadOutcome, Vec<Progress>) {
    let (_tx, rx) = watch::channel(None);
    run_with(dl, artifact, rx).await
}

async fn run_with(
    dl: &Downloader,
    artifact: &ModelArtifact,
    stop: watch::Receiver<Option<Stop>>,
) -> (DownloadOutcome, Vec<Progress>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let outcome = dl
        .run(artifact.clone(), stop, move |p| {
            sink.lock().unwrap().push(p)
        })
        .await;
    let progress = seen.lock().unwrap().clone();
    (outcome, progress)
}

fn rows(dir: &Dir) -> Vec<DownloadRow> {
    HistoryStore::open(&dir.database())
        .unwrap()
        .list_downloads()
        .unwrap()
}

fn blob(dir: &Dir, bytes: &[u8]) -> PathBuf {
    dir.models()
        .join("blobs")
        .join(format!("sha256-{}", sha(bytes)))
}

fn part(dir: &Dir, bytes: &[u8]) -> PathBuf {
    dir.models()
        .join("partial")
        .join(format!("{}.part", sha(bytes)))
}

fn seed_part(dir: &Dir, bytes: &[u8], prefix: usize) {
    let storage = Storage::open(&dir.models()).unwrap();
    let (mut file, _) = storage.open_part(&sha(bytes), bytes.len() as u64).unwrap();
    file.write_all(&bytes[..prefix]).unwrap();
}

#[tokio::test]
async fn a_fresh_download_is_verified_published_and_reported() {
    let dir = Dir::new();
    let bytes = content(300_000);
    let body = Reply::Bytes {
        code: 200,
        body: bytes.clone(),
        headers: vec![],
    };
    let server = Server::start(|_| vec![route(FILE_LINE, body)]);
    let (outcome, progress) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert_eq!(outcome.end, DownloadEnd::Completed, "{outcome:?}");
    assert_eq!(fs::read(blob(&dir, &bytes)).unwrap(), bytes);
    assert!(!part(&dir, &bytes).exists());
    assert_eq!(
        progress.last().map(|p| p.bytes_done),
        Some(bytes.len() as u64)
    );
    let rows = rows(&dir);
    assert_eq!(
        (rows.len(), rows[0].state, rows[0].bytes_done),
        (1, DownloadState::Completed, 300_000)
    );
    assert!(server.last().headers.iter().any(|h| h == "range: bytes=0-"));
}

#[tokio::test]
async fn a_partial_file_resumes_with_a_range_and_a_206() {
    let dir = Dir::new();
    let bytes = content(300_000);
    seed_part(&dir, &bytes, 100_000);
    let reply = ranged(100_000, &bytes[100_000..], 300_000);
    let server = Server::start(|_| vec![route(FILE_LINE, reply)]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert_eq!(outcome.end, DownloadEnd::Completed, "{outcome:?}");
    assert_eq!(fs::read(blob(&dir, &bytes)).unwrap(), bytes);
    assert!(
        server
            .last()
            .headers
            .iter()
            .any(|h| h == "range: bytes=100000-")
    );
}

#[tokio::test]
async fn a_server_that_ignores_the_range_restarts_from_zero() {
    let dir = Dir::new();
    let bytes = content(200_000);
    seed_part(&dir, &bytes, 50_000);
    let body = Reply::Bytes {
        code: 200,
        body: bytes.clone(),
        headers: vec![],
    };
    let server = Server::start(|_| vec![route(FILE_LINE, body)]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert_eq!(outcome.end, DownloadEnd::Completed, "{outcome:?}");
    assert_eq!(fs::read(blob(&dir, &bytes)).unwrap(), bytes);
}

#[tokio::test]
async fn a_bad_content_range_fails_and_keeps_the_partial_file() {
    let dir = Dir::new();
    let bytes = content(200_000);
    seed_part(&dir, &bytes, 50_000);
    // Starts at the wrong offset.
    let reply = ranged(60_000, &bytes[60_000..], 200_000);
    let server = Server::start(|_| vec![route(FILE_LINE, reply)]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert!(
        matches!(
            outcome.end,
            DownloadEnd::Failed {
                code: "bad_response",
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(fs::metadata(part(&dir, &bytes)).unwrap().len(), 50_000);
    assert!(!blob(&dir, &bytes).exists());
    assert_eq!(rows(&dir)[0].state, DownloadState::Failed);
}

#[tokio::test]
async fn a_stream_longer_than_the_published_size_fails_and_discards_the_partial() {
    let dir = Dir::new();
    let bytes = content(100_000);
    let mut longer = bytes.clone();
    longer.extend(content(5_000));
    let reply = Reply::Bytes {
        code: 206,
        body: longer,
        headers: vec![("Content-Range".into(), "bytes 0-99999/100000".into())],
    };
    let server = Server::start(|_| vec![route(FILE_LINE, reply)]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert!(
        matches!(
            outcome.end,
            DownloadEnd::Failed {
                code: "too_large",
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(!part(&dir, &bytes).exists());
    assert!(!blob(&dir, &bytes).exists());
}

#[tokio::test]
async fn a_digest_mismatch_deletes_the_partial_and_publishes_nothing() {
    let dir = Dir::new();
    let bytes = content(100_000);
    let mut wrong = bytes.clone();
    wrong[500] ^= 0xff;
    let body = Reply::Bytes {
        code: 200,
        body: wrong,
        headers: vec![],
    };
    let server = Server::start(|_| vec![route(FILE_LINE, body)]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert!(
        matches!(
            outcome.end,
            DownloadEnd::Failed {
                code: "integrity",
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(!part(&dir, &bytes).exists());
    assert!(!blob(&dir, &bytes).exists());
    let rows = rows(&dir);
    assert_eq!(
        (rows[0].state, rows[0].failure_code.as_deref()),
        (DownloadState::Failed, Some("integrity"))
    );
}

#[tokio::test]
async fn a_partial_file_that_already_has_every_byte_needs_no_network() {
    let dir = Dir::new();
    let bytes = content(80_000);
    seed_part(&dir, &bytes, 80_000);
    let server = Server::start(|_| vec![]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert_eq!(outcome.end, DownloadEnd::Completed, "{outcome:?}");
    assert!(server.lines().is_empty());
    assert_eq!(fs::read(blob(&dir, &bytes)).unwrap(), bytes);
}

#[tokio::test]
async fn an_already_published_file_completes_without_the_network() {
    let dir = Dir::new();
    let bytes = content(80_000);
    let first = Server::start(|_| {
        vec![route(
            FILE_LINE,
            Reply::Bytes {
                code: 200,
                body: bytes.clone(),
                headers: vec![],
            },
        )]
    });
    run(&downloader(&first, &dir), &artifact(&bytes)).await;
    let second = Server::start(|_| vec![]);
    let (outcome, _) = run(&downloader(&second, &dir), &artifact(&bytes)).await;
    assert_eq!(outcome.end, DownloadEnd::Completed);
    assert!(second.lines().is_empty());
}

#[tokio::test]
async fn pause_and_cancel_keep_the_partial_file_and_resume_finishes() {
    let dir = Dir::new();
    let bytes = content(150_000);
    let body = Reply::Bytes {
        code: 200,
        body: bytes.clone(),
        headers: vec![],
    };
    let server = Server::start(|_| vec![route(FILE_LINE, body)]);
    let dl = downloader(&server, &dir);

    let (tx, rx) = watch::channel(Some(Stop::Pause));
    let (outcome, _) = run_with(&dl, &artifact(&bytes), rx).await;
    assert!(
        matches!(outcome.end, DownloadEnd::Paused { code: None, .. }),
        "{outcome:?}"
    );
    assert_eq!(rows(&dir)[0].state, DownloadState::Paused);
    drop(tx);

    let (tx, rx) = watch::channel(Some(Stop::Cancel));
    let (outcome, _) = run_with(&dl, &artifact(&bytes), rx).await;
    assert_eq!(outcome.end, DownloadEnd::Cancelled);
    assert_eq!(rows(&dir)[0].state, DownloadState::Cancelled);
    assert!(part(&dir, &bytes).exists());
    drop(tx);

    let (outcome, _) = run(&dl, &artifact(&bytes)).await;
    assert_eq!(outcome.end, DownloadEnd::Completed);
    assert_eq!(rows(&dir).len(), 1, "resume reuses the row");
}

#[tokio::test]
async fn a_second_instance_is_refused_while_the_store_is_locked() {
    let dir = Dir::new();
    let bytes = content(1_000);
    let held = Storage::open(&dir.models()).unwrap();
    let server = Server::start(|_| vec![]);
    let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
    assert!(
        matches!(outcome.end, DownloadEnd::Failed { code: "in_use", .. }),
        "{outcome:?}"
    );
    assert_eq!(outcome.download_id, None);
    drop(held);
}

#[tokio::test]
async fn access_errors_are_typed_and_keep_nothing_but_a_failed_row() {
    let dir = Dir::new();
    let bytes = content(1_000);
    for (code, expected) in [
        (401, "access_required"),
        (404, "not_found"),
        (500, "bad_response"),
    ] {
        let server = Server::start(|_| vec![route(FILE_LINE, status(code))]);
        let (outcome, _) = run(&downloader(&server, &dir), &artifact(&bytes)).await;
        assert!(
            matches!(&outcome.end, DownloadEnd::Failed { code, .. } if *code == expected),
            "{outcome:?}"
        );
    }
    assert!(!blob(&dir, &bytes).exists());
}

#[tokio::test]
async fn downloads_without_a_checksum_size_or_with_shards_are_refused_unsent() {
    let dir = Dir::new();
    let bytes = content(1_000);
    let server = Server::start(|_| vec![]);
    let dl = downloader(&server, &dir);
    let mut unsafe_ones = Vec::new();
    let mut a = artifact(&bytes);
    a.sha256 = None;
    unsafe_ones.push(a);
    let mut a = artifact(&bytes);
    a.size_bytes = None;
    unsafe_ones.push(a);
    let mut a = artifact(&bytes);
    a.shards = Some(ShardInfo {
        count: 2,
        complete: true,
    });
    unsafe_ones.push(a);
    for a in unsafe_ones {
        let (outcome, _) = run(&dl, &a).await;
        assert!(
            matches!(
                outcome.end,
                DownloadEnd::Failed {
                    code: "not_downloadable",
                    ..
                }
            ),
            "{outcome:?}"
        );
    }
    assert!(server.lines().is_empty());
    assert!(rows(&dir).is_empty());
}

#[tokio::test]
async fn too_much_partial_data_is_refused_before_any_request() {
    let dir = Dir::new();
    let mut a = artifact(&content(10));
    a.size_bytes = Some(70 * 1024 * 1024 * 1024);
    let server = Server::start(|_| vec![]);
    let (outcome, _) = run(&downloader(&server, &dir), &a).await;
    assert!(
        matches!(
            outcome.end,
            DownloadEnd::Failed {
                code: "partial_budget",
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(server.lines().is_empty());
}

#[tokio::test]
async fn listing_reconciles_and_discard_removes_the_partial_and_the_row() {
    let dir = Dir::new();
    let bytes = content(100_000);
    seed_part(&dir, &bytes, 40_000);
    let server = Server::start(|_| vec![route(FILE_LINE, status(500))]);
    let dl = downloader(&server, &dir);
    let (outcome, _) = run(&dl, &artifact(&bytes)).await;
    let id = outcome.download_id.unwrap();
    // Simulate a crash: the row says running while nothing is.
    let store = HistoryStore::open(&dir.database()).unwrap();
    store
        .update_download(id, DownloadState::Running, 40_000, None, 1)
        .unwrap();
    assert!(
        dl.discard(id)
            .await
            .unwrap_err()
            .contains("Pause or cancel")
    );
    let listed = dl.list().await.unwrap();
    assert_eq!(
        (listed[0].state, listed[0].bytes_done),
        (DownloadState::Paused, 40_000)
    );
    dl.discard(id).await.unwrap();
    assert!(dl.list().await.unwrap().is_empty());
    assert!(!part(&dir, &bytes).exists());
}

#[tokio::test]
async fn a_dropped_stop_sender_pauses_instead_of_spinning() {
    let dir = Dir::new();
    let bytes = content(150_000);
    let body = Reply::Bytes {
        code: 200,
        body: bytes.clone(),
        headers: vec![],
    };
    let server = Server::start(|_| vec![route(FILE_LINE, body)]);
    let (tx, rx) = watch::channel(None);
    drop(tx);
    let (outcome, _) = tokio::time::timeout(
        Duration::from_secs(20),
        run_with(&downloader(&server, &dir), &artifact(&bytes), rx),
    )
    .await
    .expect("must not hang");
    assert!(
        matches!(
            outcome.end,
            DownloadEnd::Paused { .. } | DownloadEnd::Completed
        ),
        "{outcome:?}"
    );
}
