use super::*;
use crate::test_server::{Reply, Server, json_reply, route, status};
use serde_json::json;

const NATIVE: &str = "GET /api/v0/models HTTP/1.1";
const COMPAT: &str = "GET /v1/models HTTP/1.1";

fn adapter(server: &Server) -> LmStudioAdapter {
    LmStudioAdapter::new(server.port()).unwrap()
}

#[tokio::test]
async fn the_native_listing_reports_load_state_and_context() {
    let server = Server::start(|_| {
        vec![route(
            NATIVE,
            json_reply(json!({"object":"list","data":[
                {"id":"qwen2.5-7b","object":"model","type":"llm","arch":"qwen2",
                 "compatibility_type":"gguf","quantization":"Q4_K_M","state":"loaded",
                 "max_context_length":32768,"loaded_context_length":4096,"future":true},
                {"id":"embed-1","type":"embeddings","state":"not-loaded","max_context_length":0},
                {"id":"   ","state":"loaded"},
                {"id":"mystery","state":"warming-up"}
            ]})),
        )]
    });
    let found = adapter(&server).discover().await.unwrap();
    assert_eq!(found.source, Source::Native);
    assert_eq!(
        found.models.len(),
        3,
        "a model with a blank name is dropped"
    );
    let first = &found.models[0];
    assert_eq!(first.state, ModelState::Loaded);
    assert_eq!(
        (first.max_context_length, first.loaded_context_length),
        (Some(32768), Some(4096))
    );
    assert_eq!(first.quantization.as_deref(), Some("Q4_K_M"));
    assert_eq!(found.models[1].state, ModelState::NotLoaded);
    assert_eq!(
        found.models[1].max_context_length, None,
        "zero is not a length"
    );
    assert_eq!(
        found.models[2].state,
        ModelState::Unknown,
        "unrecognized state stays unknown"
    );
    assert_eq!(
        server.lines(),
        vec![NATIVE.to_string()],
        "one read-only request"
    );
}

#[tokio::test]
async fn an_older_server_falls_back_to_names_with_unknown_state() {
    let server = Server::start(|_| {
        vec![
            route(NATIVE, status(404)),
            route(
                COMPAT,
                json_reply(json!({"data":[{"id":"a","object":"model"}]})),
            ),
        ]
    });
    let found = adapter(&server).discover().await.unwrap();
    assert_eq!(found.source, Source::Compatible);
    assert_eq!(found.models[0].state, ModelState::Unknown);
}

#[tokio::test]
async fn errors_are_typed_and_never_echo_the_response() {
    let server = Server::start(|_| vec![route(NATIVE, status(401))]);
    assert_eq!(
        adapter(&server).discover().await,
        Err(LocalError::AuthRequired)
    );
    let server = Server::start(|_| vec![route(NATIVE, status(500))]);
    assert_eq!(
        adapter(&server).discover().await,
        Err(LocalError::Status(500))
    );
    let server = Server::start(|_| {
        vec![route(
            NATIVE,
            Reply::Json {
                body: "not json".into(),
                headers: vec![],
            },
        )]
    });
    assert_eq!(
        adapter(&server).discover().await,
        Err(LocalError::BadResponse)
    );
    let server = Server::start(|_| vec![route(NATIVE, Reply::DeclaredOversize)]);
    assert_eq!(adapter(&server).discover().await, Err(LocalError::TooLarge));
    let server = Server::start(|_| vec![route(NATIVE, Reply::StreamedOversize)]);
    assert_eq!(adapter(&server).discover().await, Err(LocalError::TooLarge));
    let redirect =
        Server::start(|_| vec![route(NATIVE, Reply::Redirect("http://127.0.0.1:9/".into()))]);
    assert_eq!(
        adapter(&redirect).discover().await,
        Err(LocalError::Status(302))
    );
}

#[tokio::test]
async fn nothing_listening_is_unreachable_and_port_zero_is_refused() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    assert_eq!(
        LmStudioAdapter::new(port).unwrap().discover().await,
        Err(LocalError::Unreachable)
    );
    assert!(LmStudioAdapter::new(0).is_err());
}

#[tokio::test]
async fn a_huge_listing_is_truncated_and_labels_are_cleaned() {
    let many: Vec<_> = (0..MAX_MODELS + 5)
        .map(|i| json!({"id": format!("m{i}\u{1b}[31m"), "state": "not-loaded"}))
        .collect();
    let server = Server::start(|_| vec![route(NATIVE, json_reply(json!({ "data": many })))]);
    let found = adapter(&server).discover().await.unwrap();
    assert!(found.truncated);
    assert_eq!(found.models.len(), MAX_MODELS);
    assert_eq!(found.models[0].id, "m0[31m");
}
