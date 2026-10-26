// SPDX-License-Identifier: GPL-3.0-or-later
//! A managed llama.cpp runtime as a lab target. The process is owned by the supervisor; the
//! cold load is a callback so this crate never reaches into process control itself.

use super::{LabTarget, PassError, PassOutput, Workload};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use workstation_core::experiments::Settings;
use workstation_runtimes::llamacpp::client::{ChatRequest, LlamaClient, LlamaError, StreamEvent};

/// What a cold load reports: the time to Ready and the new process group, because a restart
/// replaces the process the load sampler has to watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColdLoaded {
    pub ready_in: Duration,
    pub pgid: Option<i32>,
}

pub type ColdFuture = Pin<Box<dyn Future<Output = Result<ColdLoaded, String>> + Send>>;

pub struct LlamaTarget {
    client: LlamaClient,
    alias: String,
    pgid: Option<i32>,
    /// Stop and start the owned runtime. It must watch the flag, finish within the pass
    /// deadline and return the time until Ready with the new process group.
    cold: Box<dyn FnMut(Arc<AtomicBool>) -> ColdFuture + Send>,
}

impl LlamaTarget {
    pub fn new(
        client: LlamaClient,
        alias: String,
        pgid: Option<i32>,
        cold: Box<dyn FnMut(Arc<AtomicBool>) -> ColdFuture + Send>,
    ) -> Self {
        Self {
            client,
            alias,
            pgid,
            cold,
        }
    }
}

fn map(error: LlamaError) -> PassError {
    match error {
        LlamaError::Cancelled => PassError::Cancelled,
        other => PassError::Failed(other.to_string()),
    }
}

impl LabTarget for LlamaTarget {
    fn owned_pgid(&self) -> Option<i32> {
        self.pgid
    }

    async fn cold_load(&mut self, cancel: &Arc<AtomicBool>) -> Result<Duration, PassError> {
        let loaded = (self.cold)(cancel.clone())
            .await
            .map_err(PassError::Failed)?;
        self.pgid = loaded.pgid;
        Ok(loaded.ready_in)
    }

    async fn generate(
        &mut self,
        workload: &Workload,
        cancel: &Arc<AtomicBool>,
    ) -> Result<PassOutput, PassError> {
        let request = ChatRequest {
            model: &self.alias,
            prompt: workload.prompt(),
            max_tokens: workload.max_tokens(),
            seed: workload.seed(),
            temperature: 0.0,
            // Re-process the prompt every time so passes are comparable.
            cache_prompt: false,
        };
        let mut output = PassOutput::default();
        let started = Instant::now();
        let end = self
            .client
            .stream_chat(
                &request,
                Duration::from_secs(170),
                cancel,
                |event| match event {
                    StreamEvent::Content { at, .. } => {
                        output
                            .stream
                            .push_content(at.saturating_duration_since(started));
                    }
                    StreamEvent::Reasoning { .. } => output.stream.reasoning_events += 1,
                },
            )
            .await
            .map_err(map)?;
        // One SSE data line is one event; this client never combines events.
        output.stream.events_merged = false;
        let timings = end.timings.unwrap_or_default();
        let usage = end.usage;
        output.generated_tokens = timings.predicted_n.or(usage.map(|u| u.completion_tokens));
        output.prompt_tokens = timings.prompt_n.or(usage.map(|u| u.prompt_tokens));
        output.stream.reported_tokens = output.generated_tokens;
        output.prompt_ms = timings.prompt_ms;
        output.predicted_ms = timings.predicted_ms;
        Ok(output)
    }

    async fn observed(&mut self) -> Settings {
        let props = self.client.props().await.ok();
        Settings {
            context_length: props
                .and_then(|p| p.n_ctx)
                .and_then(|n| u32::try_from(n).ok()),
            ..Settings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    /// One-shot loopback server: answers every connection with `reply`.
    async fn serve(reply: String) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let _ = socket.read(&mut buffer).await;
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        port
    }

    fn sse(events: usize, predicted: u64) -> String {
        let mut body = String::new();
        for i in 0..events {
            body += &format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"t{i}\"}}}}]}}\n\n");
        }
        body += &format!(
            "data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"timings\":{{\"prompt_n\":30,\"prompt_ms\":60.0,\"predicted_n\":{predicted},\"predicted_ms\":240.0}}}}\n\n"
        );
        body += "data: [DONE]\n\n";
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
        )
    }

    fn target(port: u16) -> LlamaTarget {
        LlamaTarget::new(
            LlamaClient::new(port).unwrap(),
            "alias".into(),
            Some(4242),
            Box::new(|_| {
                Box::pin(async {
                    Ok(ColdLoaded {
                        ready_in: Duration::from_millis(900),
                        pgid: Some(777),
                    })
                })
            }),
        )
    }

    #[tokio::test]
    async fn a_matching_stream_gives_counter_speeds_and_a_valid_itl() {
        let mut target = target(serve(sse(12, 12)).await);
        let output = target
            .generate(&Workload::standard(), &Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert_eq!(output.generated_tokens, Some(12));
        assert_eq!(output.prompt_tokens, Some(30));
        assert_eq!(output.predicted_ms, Some(240.0));
        assert_eq!(output.stream.content_arrivals.len(), 12);
        assert!(output.stream.itl().is_ok());
        assert_eq!(target.owned_pgid(), Some(4242));
        assert_eq!(
            target.cold_load(&Arc::default()).await,
            Ok(Duration::from_millis(900))
        );
        assert_eq!(
            target.owned_pgid(),
            Some(777),
            "a restart replaces the watched group"
        );
    }

    #[tokio::test]
    async fn events_that_do_not_match_the_server_count_leave_itl_unknown() {
        let mut target = target(serve(sse(3, 9)).await);
        let output = target
            .generate(&Workload::standard(), &Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert_eq!(output.generated_tokens, Some(9));
        assert!(
            output
                .stream
                .itl()
                .unwrap_err()
                .contains("3 stream events for 9")
        );
    }

    #[tokio::test]
    async fn a_dead_port_fails_and_a_set_cancel_flag_is_reported_as_cancelled() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut target = target(port);
        let failed = target
            .generate(&Workload::standard(), &Arc::new(AtomicBool::new(false)))
            .await;
        assert!(matches!(failed, Err(PassError::Failed(_))));
        let cancelled = target
            .generate(&Workload::standard(), &Arc::new(AtomicBool::new(true)))
            .await;
        assert_eq!(cancelled.unwrap_err(), PassError::Cancelled);
        assert_eq!(target.observed().await, Settings::default());
    }
}
