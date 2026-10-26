// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{Value, json};

use super::*;
use crate::test_server::*;

const SEARCH_LINE: &str =
    "GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30 HTTP/1.1";
const SEARCH_QWEN_LINE: &str =
    "GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30&search=qwen+coder HTTP/1.1";
const DETAIL_LINE: &str = "GET /api/models/owner/name?blobs=true HTTP/1.1";
const REV: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_REV: &str = "fedcba9876543210fedcba9876543210fedcba98";
const SHA256: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

fn search_row(id: &str) -> Value {
    json!({ "id": id, "downloads": 5, "likes": 2, "gated": false })
}

fn detail_with(id: &str, sha: &str, card: Value, tags: Value, siblings: Value) -> Value {
    json!({
        "id": id,
        "sha": sha,
        "gated": "auto",
        "cardData": card,
        "tags": tags,
        "siblings": siblings,
    })
}

fn qwen_detail(id: &str, sha: &str) -> Value {
    detail_with(
        id,
        sha,
        json!({ "license": "apache-2.0", "base_model": "Qwen/Qwen2.5-Coder-7B-Instruct" }),
        json!([]),
        json!([
            { "rfilename": "m-Q4_K_M.gguf", "size": 4683073536u64,
              "lfs": { "sha256": SHA256, "size": 4683073536u64 } },
            { "rfilename": "README.md", "size": 3 },
            { "rfilename": "model.safetensors", "size": 9 }
        ]),
    )
}

async fn search_with(reply: Reply) -> Result<CatalogPage, HfError> {
    let server = Server::start(|_| vec![route(SEARCH_LINE, reply)]);
    server.client().search("", None).await
}

#[tokio::test]
async fn search_builds_the_expected_query_and_lists_valid_repos_only() {
    let server = Server::start(|_| {
        vec![route(
            SEARCH_QWEN_LINE,
            json_reply(json!([
                search_row("a/b"),
                { "modelId": "c/d" },
                { "id": "../bad" },
                { "id": "x" },
            ])),
        )]
    });
    let page = server.client().search("qwen coder", None).await.unwrap();

    let ids: Vec<_> = page
        .repos
        .iter()
        .map(|repo| repo.repo_id.as_str())
        .collect();
    assert_eq!(ids, ["a/b", "c/d"]);
    let first = &page.repos[0];
    assert_eq!(first.downloads, Some(5));
    assert_eq!(first.likes, Some(2));
    assert_eq!(first.gated, GatedAccess::Open);
    assert_eq!(page.repos[1].gated, GatedAccess::Unknown);

    let line = server.last().line;
    assert!(line.starts_with("GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30"));
    assert!(line.contains("search=qwen+coder"), "got {line}");

    let headers = server.last().headers;
    assert!(
        headers.iter().any(|h| h == "accept: application/json"),
        "{headers:?}"
    );
    assert!(
        headers
            .iter()
            .any(|h| h.starts_with("user-agent: local-ai-workstation/"))
    );
    for forbidden in ["authorization:", "cookie:", "referer:"] {
        assert!(
            !headers.iter().any(|h| h.starts_with(forbidden)),
            "{forbidden} sent: {headers:?}"
        );
    }
}

#[test]
fn next_cursor_accepts_only_same_origin_next_links_with_valid_cursors() {
    let client = HuggingFaceClient::for_test(4242);
    let same = "http://127.0.0.1:4242";
    let next = |target: &str| format!("<{target}>; rel=\"next\"");

    assert_eq!(
        client.next_cursor(&next(&format!(
            "{same}/api/models?filter=gguf&cursor=abc123"
        ))),
        Some("abc123".to_owned())
    );
    assert_eq!(
        client.next_cursor(&next("http://evil.example/api/models?cursor=x")),
        None
    );
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor=a%20b"))),
        None
    );
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor=a%3Cb"))),
        None
    );
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor=a%3Eb"))),
        None
    );
    let too_long = "a".repeat(1025);
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor={too_long}"))),
        None
    );
    assert_eq!(
        client.next_cursor(&format!("<{same}/api/models?cursor=abc>; rel=\"prev\"")),
        None
    );
}

#[tokio::test]
async fn search_follows_link_cursor_only_for_same_origin() {
    let server = Server::start(|port| {
        let link =
            format!("<http://127.0.0.1:{port}/api/models?filter=gguf&cursor=abc123>; rel=\"next\"");
        vec![route(
            SEARCH_LINE,
            Reply::Json {
                body: json!([search_row("a/b")]).to_string(),
                headers: vec![("Link".to_owned(), link)],
            },
        )]
    });
    let page = server.client().search("", None).await.unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn search_rejects_bad_input_without_sending() {
    let server = Server::start(|_| Vec::new());
    let client = server.client();

    assert_eq!(
        client
            .search("bad\nquery", None)
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    let long = "q".repeat(129);
    assert_eq!(
        client.search(&long, None).await.map(|_| ()).unwrap_err(),
        HfError::InvalidRequest
    );
    assert_eq!(
        client
            .search("", Some("a b"))
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    assert!(
        server.lines().is_empty(),
        "requests sent: {:?}",
        server.lines()
    );
}

#[tokio::test]
async fn error_statuses_map_to_typed_errors() {
    assert_eq!(
        search_with(status(401)).await.unwrap_err(),
        HfError::AccessRequired
    );
    assert_eq!(
        search_with(status(403)).await.unwrap_err(),
        HfError::AccessRequired
    );
    assert_eq!(
        search_with(status(404)).await.unwrap_err(),
        HfError::NotFound
    );
    assert_eq!(
        search_with(with_header(429, "Retry-After", "120"))
            .await
            .unwrap_err(),
        HfError::RateLimited {
            retry_after_secs: Some(120)
        }
    );
    assert_eq!(
        search_with(with_header(429, "Retry-After", "99999"))
            .await
            .unwrap_err(),
        HfError::RateLimited {
            retry_after_secs: Some(3600)
        }
    );
    assert_eq!(
        search_with(with_header(
            429,
            "Retry-After",
            "Wed, 21 Oct 2015 07:28:00 GMT"
        ))
        .await
        .unwrap_err(),
        HfError::RateLimited {
            retry_after_secs: None
        }
    );
    assert_eq!(
        search_with(status(500)).await.unwrap_err(),
        HfError::BadResponse
    );
    let not_json = Reply::Json {
        body: "not json".to_owned(),
        headers: Vec::new(),
    };
    assert_eq!(
        search_with(not_json).await.unwrap_err(),
        HfError::BadResponse
    );
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let server = Server::start(|_| {
        vec![
            route(
                SEARCH_LINE,
                Reply::Redirect("/api/models?filter=gguf&cursor=second".to_owned()),
            ),
            route(
                "GET /api/models?filter=gguf&cursor=second HTTP/1.1",
                json_reply(json!([search_row("a/b")])),
            ),
        ]
    });
    assert_eq!(
        server.client().search("", None).await.unwrap_err(),
        HfError::BadResponse
    );
    assert_eq!(server.lines(), [SEARCH_LINE.to_owned()]);
}

#[tokio::test]
async fn oversized_bodies_are_refused() {
    assert_eq!(
        search_with(Reply::DeclaredOversize).await.unwrap_err(),
        HfError::TooLarge
    );
    assert_eq!(
        search_with(Reply::StreamedOversize).await.unwrap_err(),
        HfError::TooLarge
    );
}

#[tokio::test]
async fn release_parses_detail_and_normalizes() {
    let server = Server::start(|_| {
        vec![route(
            DETAIL_LINE,
            json_reply(qwen_detail("owner/name", REV)),
        )]
    });
    let release = server.client().release("owner/name", None).await.unwrap();

    assert_eq!(release.repo_id, "owner/name");
    assert_eq!(release.revision, REV);
    assert_eq!(release.gated, GatedAccess::Automatic);
    assert_eq!(release.license.as_deref(), Some("apache-2.0"));
    assert_eq!(release.family.name, "Qwen2.5-Coder");
    assert_eq!(release.artifacts.len(), 1);
    let artifact = &release.artifacts[0];
    assert_eq!(artifact.filename, "m-Q4_K_M.gguf");
    assert_eq!(artifact.sha256.as_deref(), Some(SHA256));
    assert_eq!(artifact.size_bytes, Some(4683073536));
    assert_eq!(artifact.quantization_hint.as_deref(), Some("Q4_K_M"));
    assert_eq!(release.other_weight_files, 1);
    assert_eq!(release.dropped_invalid, 0);
}

#[tokio::test]
async fn release_falls_back_to_tags_for_license_and_base_model() {
    let detail = detail_with(
        "owner/name",
        REV,
        json!(null),
        json!([
            "license:mit",
            "base_model:finetune:Qwen/Qwen2.5-Coder-7B-Instruct"
        ]),
        json!([]),
    );
    let server = Server::start(|_| vec![route(DETAIL_LINE, json_reply(detail))]);
    let release = server.client().release("owner/name", None).await.unwrap();

    assert_eq!(release.license.as_deref(), Some("mit"));
    assert_eq!(release.family.name, "Qwen2.5-Coder");
    assert_eq!(
        release.family.source,
        workstation_core::models::family::FamilySource::BaseModel
    );
}

#[tokio::test]
async fn release_with_pin_uses_revision_endpoint_and_requires_matching_sha() {
    let pinned_line = format!("GET /api/models/owner/name/revision/{REV}?blobs=true HTTP/1.1");

    let ok = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("owner/name", REV)),
        )]
    });
    let release = ok.client().release("owner/name", Some(REV)).await.unwrap();
    assert_eq!(release.revision, REV);
    assert_eq!(ok.lines(), vec![pinned_line.clone()]);

    let wrong_sha = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("owner/name", OTHER_REV)),
        )]
    });
    assert_eq!(
        wrong_sha
            .client()
            .release("owner/name", Some(REV))
            .await
            .unwrap_err(),
        HfError::BadResponse
    );

    let other_repo = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("other/repo", REV)),
        )]
    });
    assert_eq!(
        other_repo
            .client()
            .release("owner/name", Some(REV))
            .await
            .unwrap_err(),
        HfError::BadResponse
    );

    let case_only = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("Owner/Name", REV)),
        )]
    });
    let release = case_only
        .client()
        .release("owner/name", Some(REV))
        .await
        .unwrap();
    // The requested spelling is kept so cache lookups stay consistent.
    assert_eq!(release.repo_id, "owner/name");
}

#[tokio::test]
async fn release_rejects_invalid_requests_without_sending() {
    let server = Server::start(|_| Vec::new());
    let client = server.client();
    let upper = REV.to_ascii_uppercase();

    for repo in ["no-owner", "a/b/c", "../x"] {
        assert_eq!(
            client.release(repo, None).await.map(|_| ()).unwrap_err(),
            HfError::InvalidRequest,
            "repo {repo}"
        );
    }
    assert_eq!(
        client
            .release("owner/name", Some("main"))
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    assert_eq!(
        client
            .release("owner/name", Some(&upper))
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    assert!(
        server.lines().is_empty(),
        "requests sent: {:?}",
        server.lines()
    );
}

#[tokio::test]
async fn release_with_hostile_filenames_drops_them() {
    let detail = detail_with(
        "owner/name",
        REV,
        json!({}),
        json!([]),
        json!([
            { "rfilename": "../evil.gguf", "size": 1 },
            { "rfilename": "ok.gguf", "size": 2 },
        ]),
    );
    let server = Server::start(|_| vec![route(DETAIL_LINE, json_reply(detail))]);
    let release = server.client().release("owner/name", None).await.unwrap();
    let names: Vec<_> = release
        .artifacts
        .iter()
        .map(|a| a.filename.as_str())
        .collect();
    assert_eq!(names, ["ok.gguf"]);
    assert_eq!(release.dropped_invalid, 1);

    let siblings: Vec<Value> = (0..5001)
        .map(|i| json!({ "rfilename": format!("f{i}.txt") }))
        .collect();
    let huge = detail_with(
        "owner/name",
        REV,
        json!({}),
        json!([]),
        Value::Array(siblings),
    );
    let server = Server::start(|_| vec![route(DETAIL_LINE, json_reply(huge))]);
    assert_eq!(
        server
            .client()
            .release("owner/name", None)
            .await
            .unwrap_err(),
        HfError::TooLarge
    );
}

#[test]
fn production_client_builds_with_tls_roots() {
    assert!(HuggingFaceClient::new().is_ok());
}
