use super::*;
use crate::test_server::{Reply, Server, json_reply, route, status};
use serde_json::json;

const HEALTH: &str = "GET /health HTTP/1.1";
const MODELS: &str = "GET /v1/models HTTP/1.1";
const METRICS: &str = "GET /metrics HTTP/1.1";

fn text(body: &str) -> Reply {
    Reply::Bytes {
        code: 200,
        body: body.as_bytes().to_vec(),
        headers: vec![],
    }
}

const SAMPLE: &str = "\
# HELP vllm:num_requests_running Number of requests in model execution batches.
# TYPE vllm:num_requests_running gauge
vllm:num_requests_running{engine=\"0\",model_name=\"m\"} 2.0
vllm:num_requests_waiting{engine=\"0\",model_name=\"m\"} 1.0
vllm:kv_cache_usage_perc{engine=\"0\",model_name=\"m\"} 0.25
vllm:prompt_tokens_total{engine=\"0\",model_name=\"m\"} 1200.0
vllm:prompt_tokens_total{engine=\"1\",model_name=\"m\"} 300.0
vllm:generation_tokens_total{engine=\"0\",model_name=\"m\"} 77
vllm:something_else 9
vllm:num_requests_waiting{engine=\"1\"} nan
vllm:num_requests_waiting{engine=\"2\"} -4
";

#[test]
fn only_known_names_are_read_labels_are_summed_and_bad_values_ignored() {
    let m = parse_metrics(SAMPLE);
    assert_eq!(m.requests_running, Some(2.0));
    assert_eq!(
        m.requests_waiting,
        Some(1.0),
        "nan and negative series are skipped"
    );
    assert_eq!(m.kv_cache_usage, Some(0.25));
    assert_eq!(m.prompt_tokens_total, Some(1500.0));
    assert_eq!(m.generation_tokens_total, Some(77.0));
    assert_eq!(
        parse_metrics("vllm:num_requests_running abc\n"),
        Metrics::default()
    );
    assert_eq!(parse_metrics(""), Metrics::default());
}

#[test]
fn the_older_cache_gauge_name_is_read_and_the_highest_value_wins() {
    let m = parse_metrics("vllm:gpu_cache_usage_perc 0.4\nvllm:kv_cache_usage_perc 0.1\n");
    assert_eq!(m.kv_cache_usage, Some(0.4));
}

#[tokio::test]
async fn discovery_reads_health_models_and_metrics_with_three_read_only_requests() {
    let server = Server::start(|_| {
        vec![
            route(HEALTH, text("")),
            route(
                MODELS,
                json_reply(
                    json!({"data":[{"id":"meta/llama","max_model_len":8192,"owned_by":"vllm"},{"id":" "}]}),
                ),
            ),
            route(METRICS, text(SAMPLE)),
        ]
    });
    let found = VllmAdapter::new(server.port()).unwrap().discover().await;
    assert_eq!(found.healthy, Ok(()));
    assert_eq!(
        found.models,
        Ok(vec![Model {
            id: "meta/llama".into(),
            max_model_len: Some(8192)
        }])
    );
    assert_eq!(found.metrics.unwrap().requests_running, Some(2.0));
    let mut lines = server.lines();
    lines.sort();
    assert_eq!(lines, vec![HEALTH, METRICS, MODELS]);
}

#[tokio::test]
async fn each_part_fails_on_its_own() {
    let server = Server::start(|_| {
        vec![
            route(HEALTH, status(503)),
            route(MODELS, status(401)),
            route(METRICS, Reply::DeclaredOversize),
        ]
    });
    let found = VllmAdapter::new(server.port()).unwrap().discover().await;
    assert_eq!(found.healthy, Err(LocalError::Status(503)));
    assert_eq!(found.models, Err(LocalError::AuthRequired));
    assert_eq!(found.metrics, Err(LocalError::TooLarge));
}

#[tokio::test]
async fn a_server_without_metrics_still_lists_models() {
    let server = Server::start(|_| {
        vec![
            route(HEALTH, text("")),
            route(MODELS, json_reply(json!({"data":[{"id":"a"}]}))),
        ]
    });
    let found = VllmAdapter::new(server.port()).unwrap().discover().await;
    assert_eq!(found.metrics, Err(LocalError::Status(404)));
    assert_eq!(found.models.unwrap().len(), 1);
}
