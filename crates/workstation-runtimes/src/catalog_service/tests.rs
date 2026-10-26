// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::test_server::*;
use serde_json::{Value, json};
use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

const REPO: &str = "o/n-GGUF";
const REV_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REV_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SHA256: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const SEARCH_LINE: &str =
    "GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30 HTTP/1.1";
const DETAIL_LINE: &str = "GET /api/models/o/n-GGUF?blobs=true HTTP/1.1";

/// Own database per test, removed on drop.
struct TempDb(PathBuf);
impl TempDb {
    fn new() -> Self {
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(PathBuf::from(format!(
            "/tmp/kvlm-svc-{stamp}-{nonce}/history.db"
        )))
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn service(port: u16, db: &TempDb) -> CatalogService {
    CatalogService::with_parts(HuggingFaceClient::for_test(port), db.0.clone())
}

fn search_body() -> Value {
    json!([{ "id": REPO, "downloads": 3 }])
}

fn detail(sha: &str) -> Value {
    json!({
        "id": REPO,
        "sha": sha,
        "cardData": { "license": "apache-2.0" },
        "tags": [],
        "siblings": [
            { "rfilename": "m-Q4_K_M.gguf", "size": 4683073536u64,
              "lfs": { "sha256": SHA256, "size": 4683073536u64 } }
        ],
    })
}

fn pinned_line(revision: &str) -> String {
    format!("GET /api/models/o/n-GGUF/revision/{revision}?blobs=true HTTP/1.1")
}

#[tokio::test]
async fn live_search_is_cached_and_replayable_offline() {
    let db = TempDb::new();
    let server = Server::start(|_| vec![route(SEARCH_LINE, json_reply(search_body()))]);
    let svc = service(server.port(), &db);

    let live = svc.search("", None).await.unwrap();
    assert_eq!(live.source, LoadSource::Live);
    assert_eq!(live.notice, None);
    assert_eq!(live.page.repos[0].repo_id, REPO);

    let cached = svc
        .cached_search("", None)
        .await
        .unwrap()
        .expect("cached page");
    assert!(matches!(cached.source, LoadSource::Cached { .. }));
    assert_eq!(cached.notice, None);
    assert_eq!(cached.page, live.page);

    assert!(svc.cached_search("qwen", None).await.unwrap().is_none());
}

#[tokio::test]
async fn transient_failure_falls_back_to_the_saved_page() {
    let db = TempDb::new();
    let ok = Server::start(|_| vec![route(SEARCH_LINE, json_reply(search_body()))]);
    let live = service(ok.port(), &db).search("", None).await.unwrap();

    let broken = Server::start(|_| vec![route(SEARCH_LINE, status(500))]);
    let fallback = service(broken.port(), &db).search("", None).await.unwrap();
    assert!(matches!(fallback.source, LoadSource::Cached { .. }));
    assert_eq!(fallback.page, live.page);
    let notice = fallback.notice.expect("notice");
    assert!(notice.contains("Showing results saved earlier"), "{notice}");

    let empty = TempDb::new();
    let error = service(broken.port(), &empty)
        .search("", None)
        .await
        .unwrap_err();
    assert!(error.contains("cannot use"), "{error}");
}

#[tokio::test]
async fn rate_limit_notice_mentions_retry_time() {
    let db = TempDb::new();
    let ok = Server::start(|_| vec![route(SEARCH_LINE, json_reply(search_body()))]);
    service(ok.port(), &db).search("", None).await.unwrap();

    let limited =
        Server::start(|_| vec![route(SEARCH_LINE, with_header(429, "Retry-After", "120"))]);
    let load = service(limited.port(), &db).search("", None).await.unwrap();
    assert!(matches!(load.source, LoadSource::Cached { .. }));
    let notice = load.notice.expect("notice");
    assert!(notice.contains("about 120 seconds"), "{notice}");
}

#[tokio::test]
async fn refused_connection_is_offline_and_uses_cache() {
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let db = TempDb::new();
    let ok = Server::start(|_| vec![route(SEARCH_LINE, json_reply(search_body()))]);
    let live = service(ok.port(), &db).search("", None).await.unwrap();

    let offline = service(closed, &db).search("", None).await.unwrap();
    assert!(matches!(offline.source, LoadSource::Cached { .. }));
    assert_eq!(offline.page, live.page);

    let empty = TempDb::new();
    let error = service(closed, &empty).search("", None).await.unwrap_err();
    assert!(error.contains("Cannot reach Hugging Face"), "{error}");
}

#[tokio::test]
async fn access_and_not_found_never_use_stale_data() {
    let db = TempDb::new();
    let ok = Server::start(|_| vec![route(SEARCH_LINE, json_reply(search_body()))]);
    service(ok.port(), &db).search("", None).await.unwrap();

    let denied = Server::start(|_| vec![route(SEARCH_LINE, status(401))]);
    let error = service(denied.port(), &db)
        .search("", None)
        .await
        .unwrap_err();
    assert!(error.contains("needs an access token"), "{error}");

    let missing = Server::start(|_| vec![route(SEARCH_LINE, status(404))]);
    let error = service(missing.port(), &db)
        .search("", None)
        .await
        .unwrap_err();
    assert!(error.contains("no such repository"), "{error}");
}

#[tokio::test]
async fn release_live_is_cached_and_fallback_prefers_pin() {
    let db = TempDb::new();
    let live_server = Server::start(|_| {
        vec![
            route(DETAIL_LINE, json_reply(detail(REV_A))),
            route(&pinned_line(REV_A), json_reply(detail(REV_A))),
        ]
    });
    let svc = service(live_server.port(), &db);

    let live = svc.release(REPO, None).await.unwrap();
    assert_eq!(live.source, LoadSource::Live);
    assert_eq!(live.notice, None);
    assert_eq!(live.release.revision, REV_A);
    let pinned_live = svc.release(REPO, Some(REV_A)).await.unwrap();
    assert_eq!(pinned_live.source, LoadSource::Live);

    let down = Server::start(|_| {
        vec![
            route(DETAIL_LINE, status(500)),
            route(&pinned_line(REV_A), status(500)),
            route(&pinned_line(REV_B), status(500)),
        ]
    });
    let fallback = service(down.port(), &db);

    let latest = fallback.release(REPO, None).await.unwrap();
    assert!(matches!(latest.source, LoadSource::Cached { .. }));
    assert_eq!(latest.release, live.release);
    let notice = latest.notice.expect("notice");
    assert!(notice.contains("Showing files saved earlier"), "{notice}");

    let pinned = fallback.release(REPO, Some(REV_A)).await.unwrap();
    assert!(matches!(pinned.source, LoadSource::Cached { .. }));
    assert_eq!(pinned.release.revision, REV_A);

    assert!(fallback.release(REPO, Some(REV_B)).await.is_err());
}

#[tokio::test]
async fn unwritable_cache_does_not_hide_a_live_result() {
    let db = TempDb(PathBuf::from("/proc/kvlm-no/history.db"));
    let ok = Server::start(|_| {
        vec![
            route(SEARCH_LINE, json_reply(search_body())),
            route(DETAIL_LINE, json_reply(detail(REV_A))),
        ]
    });
    let svc = service(ok.port(), &db);

    let search = svc.search("", None).await.unwrap();
    assert_eq!(search.source, LoadSource::Live);
    assert_eq!(
        search.notice.as_deref(),
        Some("Could not save this result for offline use.")
    );

    let release = svc.release(REPO, None).await.unwrap();
    assert_eq!(release.source, LoadSource::Live);
    assert_eq!(
        release.notice.as_deref(),
        Some("Could not save this result for offline use.")
    );
}
