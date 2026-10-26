// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed messages keep GTK objects out of blocking hardware probes.

use crate::fit_workflow::{FitReport, FitRequest, estimate, fit_report, parse_scenario};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::mpsc;
use workstation_bench::{
    history::{self, PersistedRun},
    runner::BenchmarkOutcome,
};
use workstation_core::db::HistoryEntry;
use workstation_core::hardware::{HardwareProfile, resources::ResourceSummary};
use workstation_core::{
    fit::{gguf_fit::LayoutAssumption, memory_math::KvCacheType},
    models::artifact::ModelArtifact,
};
use workstation_runtimes::catalog_service::{
    CatalogLoad, CatalogService, InspectLoad, LoadSource, ReleaseLoad,
};
use workstation_runtimes::ollama::{
    Discovery, GenerationProgress, InstalledModel, ModelInspection, OllamaAdapter,
};

#[derive(Debug)]
pub enum CoreCommand {
    CheckBenchmarkVerification {
        id: u64,
        model_name: String,
        model_digest: String,
    },
    LoadBenchmarkHistory {
        id: u64,
    },
    DetectHardware,
    DiscoverOllama,
    InspectOllama {
        id: u64,
        model: String,
    },
    EstimateFit(FitRequest),
    /// Explicit network search; falls back to saved results when offline.
    SearchCatalog {
        id: u64,
        query: String,
        cursor: Option<String>,
    },
    /// Saved first page only; never touches the network.
    LoadCachedCatalog {
        id: u64,
        query: String,
    },
    /// Explicit network fetch of one repository's files; `pinned` is a commit id.
    ExpandRelease {
        id: u64,
        repo_id: String,
        pinned: Option<String>,
    },
    /// Explicit fetch of one file's header (first 16 MiB at most) and a fit estimate.
    InspectArtifact {
        id: u64,
        artifact: Box<ModelArtifact>,
    },
    RunQuickBenchmark {
        id: u64,
        model: InstalledModel,
        cancel: tokio::sync::watch::Receiver<bool>,
    },
}

#[derive(Debug)]
pub enum UiEvent {
    BenchmarkVerificationChecked {
        id: u64,
        result: Result<Option<i64>, String>,
    },
    BenchmarkHistoryLoaded {
        id: u64,
        result: Result<Vec<HistoryEntry>, String>,
    },
    BenchmarkHistorySaved {
        benchmark_id: u64,
        result: Result<i64, String>,
    },
    BenchmarkFinished {
        id: u64,
        outcome: BenchmarkOutcome,
        resources: Option<Box<ResourceSummary>>,
    },
    OllamaDiscovered(Box<Discovery>),
    OllamaFailed(String),
    OllamaInspected {
        id: u64,
        result: Result<Box<ModelInspection>, String>,
    },
    HardwareDetected(Box<HardwareProfile>),
    HardwareFailed(String),
    FitFinished {
        id: u64,
        result: Result<Box<FitReport>, String>,
    },
    /// `Ok(None)` means a cache-only request found nothing saved.
    CatalogLoaded {
        id: u64,
        result: Result<Option<Box<CatalogLoad>>, String>,
    },
    ReleaseLoaded {
        id: u64,
        result: Result<Box<ReleaseLoad>, String>,
    },
    ArtifactInspected {
        id: u64,
        result: Result<Box<ArtifactDetails>, String>,
    },
}

/// A file's header facts and the conditional fit for the default scenario.
#[derive(Debug)]
pub struct ArtifactDetails {
    pub report: FitReport,
    pub source: LoadSource,
    pub notice: Option<String>,
}

/// 8192 context, F16 K/V, microbatch 512, uniform attention assumed. Conditional only.
async fn artifact_details(
    load: impl Future<Output = Result<InspectLoad, String>>,
    probe: Arc<impl Fn() -> HardwareProfile + Send + Sync + 'static>,
) -> Result<ArtifactDetails, String> {
    let InspectLoad {
        metadata,
        source,
        notice,
    } = load.await?;
    let scenario = parse_scenario("8192", "512", KvCacheType::F16, KvCacheType::F16)?;
    let report = tokio::task::spawn_blocking(move || {
        fit_report(
            metadata,
            &scenario,
            LayoutAssumption::UniformFullAttention,
            probe(),
        )
    })
    .await
    .map_err(|_| "Fit estimate failed. Retry.".to_string())??;
    Ok(ArtifactDetails {
        report,
        source,
        notice,
    })
}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Source of catalog data. Production uses [`CatalogService`]; tests inject fakes.
pub trait CatalogBackend: Send + Sync + 'static {
    fn search(
        &self,
        query: String,
        cursor: Option<String>,
    ) -> BoxFuture<Result<CatalogLoad, String>>;
    fn cached_search(&self, query: String) -> BoxFuture<Result<Option<CatalogLoad>, String>>;
    fn release(
        &self,
        repo_id: String,
        pinned: Option<String>,
    ) -> BoxFuture<Result<ReleaseLoad, String>>;
    fn inspect(&self, artifact: ModelArtifact) -> BoxFuture<Result<InspectLoad, String>>;
}

/// Builds the real service on first use, so startup does no catalog work.
struct LiveCatalog(std::sync::OnceLock<Result<Arc<CatalogService>, String>>);

impl LiveCatalog {
    fn service(&self) -> Result<Arc<CatalogService>, String> {
        self.0
            .get_or_init(|| CatalogService::new().map(Arc::new))
            .clone()
    }
}

impl CatalogBackend for LiveCatalog {
    fn search(
        &self,
        query: String,
        cursor: Option<String>,
    ) -> BoxFuture<Result<CatalogLoad, String>> {
        let service = self.service();
        Box::pin(async move { service?.search(&query, cursor.as_deref()).await })
    }
    fn cached_search(&self, query: String) -> BoxFuture<Result<Option<CatalogLoad>, String>> {
        let service = self.service();
        Box::pin(async move { service?.cached_search(&query, None).await })
    }
    fn release(
        &self,
        repo_id: String,
        pinned: Option<String>,
    ) -> BoxFuture<Result<ReleaseLoad, String>> {
        let service = self.service();
        Box::pin(async move { service?.release(&repo_id, pinned.as_deref()).await })
    }
    fn inspect(&self, artifact: ModelArtifact) -> BoxFuture<Result<InspectLoad, String>> {
        let service = self.service();
        Box::pin(async move { service?.inspect(&artifact).await })
    }
}

/// Aborts the wrapped task when the guarding job is dropped or aborted.
struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs a catalog job so a panic is reported for its own request id; aborting the
/// guarding job (a newer request) aborts the work too and produces no event.
async fn guarded(
    id: u64,
    failed: fn(u64) -> UiEvent,
    work: impl std::future::Future<Output = UiEvent> + Send + 'static,
) -> UiEvent {
    let task = tokio::spawn(work);
    let _abort = AbortOnDrop(task.abort_handle());
    task.await.unwrap_or_else(|_| failed(id))
}

fn catalog_failed(id: u64) -> UiEvent {
    UiEvent::CatalogLoaded {
        id,
        result: Err("Catalog worker failed. Retry.".into()),
    }
}

fn inspect_failed(id: u64) -> UiEvent {
    UiEvent::ArtifactInspected {
        id,
        result: Err("Inspection worker failed. Retry.".into()),
    }
}

fn release_failed(id: u64) -> UiEvent {
    UiEvent::ReleaseLoaded {
        id,
        result: Err("Catalog worker failed. Retry.".into()),
    }
}

pub fn start(
    runtime: &tokio::runtime::Handle,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_workers(runtime, HardwareProfile::detect, estimate)
}

#[cfg(test)]
fn start_with_probe(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_workers(runtime, probe, estimate)
}

fn start_with_workers(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
    fit: impl Fn(FitRequest) -> Result<FitReport, String> + Send + Sync + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_runtime_workers(
        runtime,
        probe,
        fit,
        || async { OllamaAdapter::new()?.discover().await },
        |model| async move { OllamaAdapter::new()?.inspect_model(&model).await },
    )
}

fn start_with_runtime_workers<D, I>(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
    fit: impl Fn(FitRequest) -> Result<FitReport, String> + Send + Sync + 'static,
    discover: impl Fn() -> D + Send + 'static,
    inspect: impl Fn(String) -> I + Send + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>)
where
    D: std::future::Future<Output = Result<Discovery, String>> + Send,
    I: std::future::Future<Output = Result<ModelInspection, String>> + Send,
{
    start_with_all_workers(
        runtime,
        probe,
        fit,
        discover,
        inspect,
        history::run,
        history::load_default,
    )
}

fn start_with_all_workers<D, I, B>(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
    fit: impl Fn(FitRequest) -> Result<FitReport, String> + Send + Sync + 'static,
    discover: impl Fn() -> D + Send + 'static,
    inspect: impl Fn(String) -> I + Send + 'static,
    run_benchmark: impl Fn(InstalledModel, tokio::sync::watch::Receiver<bool>) -> B + Send + 'static,
    load_history: impl Fn() -> Result<Vec<HistoryEntry>, String> + Send + Sync + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>)
where
    D: std::future::Future<Output = Result<Discovery, String>> + Send,
    I: std::future::Future<Output = Result<ModelInspection, String>> + Send,
    B: std::future::Future<Output = PersistedRun> + Send + 'static,
{
    start_with_catalog(
        runtime,
        probe,
        fit,
        discover,
        inspect,
        run_benchmark,
        load_history,
        Arc::new(LiveCatalog(std::sync::OnceLock::new())),
    )
}

/// Every worker is injectable so tests never touch hardware, Ollama, or the network.
#[allow(clippy::too_many_arguments)]
fn start_with_catalog<D, I, B>(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
    fit: impl Fn(FitRequest) -> Result<FitReport, String> + Send + Sync + 'static,
    discover: impl Fn() -> D + Send + 'static,
    inspect: impl Fn(String) -> I + Send + 'static,
    run_benchmark: impl Fn(InstalledModel, tokio::sync::watch::Receiver<bool>) -> B + Send + 'static,
    load_history: impl Fn() -> Result<Vec<HistoryEntry>, String> + Send + Sync + 'static,
    catalog: Arc<dyn CatalogBackend>,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>)
where
    D: std::future::Future<Output = Result<Discovery, String>> + Send,
    I: std::future::Future<Output = Result<ModelInspection, String>> + Send,
    B: std::future::Future<Output = PersistedRun> + Send + 'static,
{
    let (commands, mut command_rx) = mpsc::channel(1);
    let (events, event_rx) = async_channel::bounded(1);
    let probe = Arc::new(probe);
    let fit = Arc::new(fit);
    let load_history = Arc::new(load_history);
    runtime.spawn(async move {
        let mut pending = None;
        let mut benchmark_jobs = tokio::task::JoinSet::new();
        let mut benchmark_id = 0;
        // One live search, one saved-page read and one release fetch at a time; a newer
        // request of the same kind aborts the older. Saved-page reads never cancel a search.
        let mut search_jobs: tokio::task::JoinSet<UiEvent> = tokio::task::JoinSet::new();
        let mut cached_jobs: tokio::task::JoinSet<UiEvent> = tokio::task::JoinSet::new();
        let mut release_jobs: tokio::task::JoinSet<UiEvent> = tokio::task::JoinSet::new();
        let mut inspect_jobs: tokio::task::JoinSet<UiEvent> = tokio::task::JoinSet::new();
        loop {
            if events.is_closed() { break; }
            let command = if let Some(command) = pending.take() { command } else { tokio::select! {
                _ = events.closed() => break,
                result = benchmark_jobs.join_next(), if !benchmark_jobs.is_empty() => {
                    let PersistedRun { outcome, saved, resources } = match result {
                        Some(Ok(result)) => result,
                        _ => PersistedRun { outcome: BenchmarkOutcome::Failed { message: "Benchmark worker failed. Try again.".into(), partial: GenerationProgress::default() }, saved: Err("Benchmark worker stopped; reload history to inspect unfinished runs.".into()), resources: None },
                    };
                    if events.send(UiEvent::BenchmarkFinished { id: benchmark_id, outcome, resources: resources.map(Box::new) }).await.is_err() { break; }
                    if events.send(UiEvent::BenchmarkHistorySaved { benchmark_id, result: saved }).await.is_err() { break; }
                    continue;
                }
                result = search_jobs.join_next(), if !search_jobs.is_empty() => {
                    if let Some(Ok(event)) = result
                        && events.send(event).await.is_err() { break; }
                    continue;
                }
                result = cached_jobs.join_next(), if !cached_jobs.is_empty() => {
                    if let Some(Ok(event)) = result
                        && events.send(event).await.is_err() { break; }
                    continue;
                }
                result = release_jobs.join_next(), if !release_jobs.is_empty() => {
                    if let Some(Ok(event)) = result
                        && events.send(event).await.is_err() { break; }
                    continue;
                }
                result = inspect_jobs.join_next(), if !inspect_jobs.is_empty() => {
                    if let Some(Ok(event)) = result
                        && events.send(event).await.is_err() { break; }
                    continue;
                }
                command = command_rx.recv() => match command { Some(command) => command, None => break },
            }};
            match command {
                CoreCommand::CheckBenchmarkVerification { id, model_name, model_digest } => {
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = check_verification(model_name, model_digest) => result,
                    };
                    if events.send(UiEvent::BenchmarkVerificationChecked { id, result }).await.is_err() { break; }
                }
                CoreCommand::LoadBenchmarkHistory { id } => {
                    let load = Arc::clone(&load_history);
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = tokio::task::spawn_blocking(move || load()) => result,
                    }.unwrap_or_else(|_| Err("History worker failed. Reload to retry.".into()));
                    if events.send(UiEvent::BenchmarkHistoryLoaded { id, result }).await.is_err() { break; }
                }
                CoreCommand::SearchCatalog { id, query, cursor } => {
                    search_jobs.abort_all();
                    let catalog = Arc::clone(&catalog);
                    search_jobs.spawn(guarded(id, catalog_failed, async move {
                        let result = catalog.search(query, cursor).await.map(|load| Some(Box::new(load)));
                        UiEvent::CatalogLoaded { id, result }
                    }));
                }
                CoreCommand::LoadCachedCatalog { id, query } => {
                    cached_jobs.abort_all();
                    let catalog = Arc::clone(&catalog);
                    cached_jobs.spawn(guarded(id, catalog_failed, async move {
                        let result = catalog.cached_search(query).await.map(|load| load.map(Box::new));
                        UiEvent::CatalogLoaded { id, result }
                    }));
                }
                CoreCommand::ExpandRelease { id, repo_id, pinned } => {
                    release_jobs.abort_all();
                    let catalog = Arc::clone(&catalog);
                    release_jobs.spawn(guarded(id, release_failed, async move {
                        let result = catalog.release(repo_id, pinned).await.map(Box::new);
                        UiEvent::ReleaseLoaded { id, result }
                    }));
                }
                CoreCommand::InspectArtifact { id, artifact } => {
                    inspect_jobs.abort_all();
                    let (catalog, probe) = (Arc::clone(&catalog), Arc::clone(&probe));
                    inspect_jobs.spawn(guarded(id, inspect_failed, async move {
                        let result = artifact_details(catalog.inspect(*artifact), probe).await.map(Box::new);
                        UiEvent::ArtifactInspected { id, result }
                    }));
                }
                CoreCommand::RunQuickBenchmark { id, model, cancel } => {
                    if benchmark_jobs.is_empty() {
                        benchmark_id = id;
                        benchmark_jobs.spawn(run_benchmark(model, cancel));
                    } else {
                        let outcome = BenchmarkOutcome::Failed { message: "Another quick benchmark is still active. Retry after it finishes.".into(), partial: GenerationProgress::default() };
                        if events.send(UiEvent::BenchmarkFinished { id, outcome, resources: None }).await.is_err() { break; }
                    }
                }
                CoreCommand::DetectHardware => {
                    let probe = Arc::clone(&probe);
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = tokio::task::spawn_blocking(move || probe()) => result,
                    };
                    // Coalesce only refreshes; a queued fit request must not be discarded.
                    while let Ok(command) = command_rx.try_recv() {
                        if matches!(command, CoreCommand::DetectHardware) { continue; }
                        pending = Some(command);
                        break;
                    }
                    let event = match result {
                        Ok(profile) if profile.has_readings() => UiEvent::HardwareDetected(Box::new(profile)),
                        Ok(profile) => UiEvent::HardwareFailed(profile.issues.iter()
                            .map(|issue| format!("{}: {}", issue.component, issue.message))
                            .collect::<Vec<_>>().join("\n")),
                        Err(error) => UiEvent::HardwareFailed(format!("Hardware worker failed: {error}")),
                    };
                    if events.send(event).await.is_err() { break; }
                }
                CoreCommand::DiscoverOllama => {
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = discover() => result,
                    };
                    let event = match result {
                        Ok(discovery) => UiEvent::OllamaDiscovered(Box::new(discovery)),
                        Err(message) => UiEvent::OllamaFailed(message),
                    };
                    if events.send(event).await.is_err() { break; }
                }
                CoreCommand::InspectOllama { id, model } => {
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = inspect(model) => result.map(Box::new),
                    };
                    if events.send(UiEvent::OllamaInspected { id, result }).await.is_err() { break; }
                }
                CoreCommand::EstimateFit(request) => {
                    let id = request.id;
                    let fit = Arc::clone(&fit);
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = tokio::task::spawn_blocking(move || fit(request)) => result,
                    };
                    let result = result
                        .map_err(|_| "Model inspection worker failed. Try again with a local GGUF file.".into())
                        .and_then(|result| result)
                        .map(Box::new);
                    if events.send(UiEvent::FitFinished { id, result }).await.is_err() { break; }
                }
            }
        }
    });
    (commands, event_rx)
}

async fn check_verification(
    model_name: String,
    model_digest: String,
) -> Result<Option<i64>, String> {
    use workstation_core::{
        db::{self, HistoryStore},
        models::artifact::canonical_manifest_digest,
    };
    let digest = canonical_manifest_digest(&model_digest).ok_or("Invalid model digest.")?;
    let discovery = OllamaAdapter::new()?.discover().await?;
    let models = discovery.installed?;
    let mut selected = models.iter().filter(|model| model.name == model_name);
    let model = selected
        .next()
        .ok_or("Model is no longer installed. Refresh Runtimes.")?;
    if selected.next().is_some()
        || model.cloud_routed()
        || model.name.to_ascii_lowercase().ends_with(":cloud")
        || model.name.to_ascii_lowercase().ends_with("-cloud")
        || model
            .digest
            .as_deref()
            .and_then(canonical_manifest_digest)
            .as_ref()
            != Some(&digest)
    {
        return Err("Model identity or routing changed. Refresh Runtimes.".into());
    }
    tokio::task::spawn_blocking(move || {
        let hardware = HardwareProfile::detect();
        HistoryStore::open(&db::default_database_path()?)?.verified_run(&digest, &hardware)
    })
    .await
    .map_err(|_| "Verification worker failed. Retry the check.")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use workstation_core::hardware::{MemoryPool, ProbeIssue};

    fn snapshot() -> HardwareProfile {
        HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: Some(MemoryPool {
                total_bytes: 1024,
                available_bytes: Some(512),
            }),
            gpus: Vec::new(),
            issues: vec![ProbeIssue {
                component: "CPU".into(),
                message: "Fixture partial failure".into(),
            }],
            captured_at_unix_ms: 1,
        }
    }

    async fn receive(events: &async_channel::Receiver<UiEvent>) -> UiEvent {
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn delivers_partial_readings_and_closes_with_commands() {
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), snapshot);
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        let UiEvent::HardwareDetected(profile) = receive(&events).await else {
            panic!("Expected usable partial readings");
        };
        assert_eq!(profile.issues.len(), 1);
        assert_eq!(profile.system_ram.unwrap().available_bytes, Some(512));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn empty_readings_and_panics_become_failure_events() {
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), || {
            let mut profile = snapshot();
            profile.system_ram = None;
            profile
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        assert!(
            matches!(receive(&events).await, UiEvent::HardwareFailed(message) if message.contains("Fixture partial"))
        );
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), || {
            panic!("fixture probe panic")
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        assert!(
            matches!(receive(&events).await, UiEvent::HardwareFailed(message) if message.contains("Hardware worker failed"))
        );
    }

    #[tokio::test]
    async fn coalesces_queued_refreshes_without_blocking_the_async_worker() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), move || {
            count.fetch_add(1, Ordering::SeqCst);
            started.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            snapshot()
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        assert!(commands.try_send(CoreCommand::DetectHardware).is_err());
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::HardwareDetected(_)
        ));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn closing_window_releases_command_channel_during_a_probe() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), move || {
            started.send(()).unwrap();
            let _ = gate.lock().unwrap().recv();
            snapshot()
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        events.close();
        let closed = tokio::time::timeout(Duration::from_secs(2), commands.closed()).await;
        release.send(()).unwrap();
        assert!(closed.is_ok());
    }

    /// Catalog fake: "slow" never finishes, "panic" panics, anything else succeeds.
    struct FakeCatalog;
    fn page_for(query: &str) -> workstation_core::models::catalog::CatalogPage {
        use workstation_core::models::{catalog::RepoSummary, release::GatedAccess};
        workstation_core::models::catalog::CatalogPage {
            repos: vec![RepoSummary {
                repo_id: format!("o/{query}"),
                downloads: Some(1),
                likes: None,
                gated: GatedAccess::Open,
            }],
            next_cursor: None,
        }
    }
    impl CatalogBackend for FakeCatalog {
        fn search(
            &self,
            query: String,
            _cursor: Option<String>,
        ) -> BoxFuture<Result<CatalogLoad, String>> {
            Box::pin(async move {
                match query.as_str() {
                    "slow" => std::future::pending().await,
                    "panic" => panic!("fixture catalog panic"),
                    "fail" => Err("fixture failure".into()),
                    _ => Ok(CatalogLoad {
                        page: page_for(&query),
                        source: workstation_runtimes::catalog_service::LoadSource::Live,
                        notice: None,
                    }),
                }
            })
        }
        fn cached_search(&self, query: String) -> BoxFuture<Result<Option<CatalogLoad>, String>> {
            Box::pin(async move {
                Ok((query != "empty").then(|| CatalogLoad {
                    page: page_for(&query),
                    source: workstation_runtimes::catalog_service::LoadSource::Cached {
                        fetched_at_unix_ms: 7,
                    },
                    notice: None,
                }))
            })
        }
        fn release(
            &self,
            repo_id: String,
            _pinned: Option<String>,
        ) -> BoxFuture<Result<ReleaseLoad, String>> {
            Box::pin(async move {
                if repo_id == "slow/slow" {
                    std::future::pending::<()>().await;
                }
                Err("fixture release failure".into())
            })
        }
        fn inspect(&self, artifact: ModelArtifact) -> BoxFuture<Result<InspectLoad, String>> {
            Box::pin(async move {
                match artifact.filename.as_str() {
                    "slow.gguf" => std::future::pending().await,
                    "panic.gguf" => panic!("fixture inspect panic"),
                    "bad.gguf" => Err("fixture header failure".into()),
                    _ => Ok(InspectLoad {
                        metadata: workstation_core::models::gguf::GgufMetadata {
                            file_bytes: 100,
                            tensor_count: 1,
                            tensor_bytes: 4,
                            alignment: 32,
                            architecture: Some("qwen2".into()),
                            attention: None,
                            warnings: vec![],
                        },
                        source: LoadSource::Live,
                        notice: None,
                    }),
                }
            })
        }
    }

    fn artifact_named(name: &str) -> Box<ModelArtifact> {
        use workstation_core::models::artifact::ArtifactFormat;
        Box::new(ModelArtifact {
            artifact_id: format!("hf:o/n@{}:{name}", "a".repeat(40)),
            repo_id: "o/n".into(),
            revision: "a".repeat(40),
            filename: name.into(),
            format: ArtifactFormat::Gguf,
            quantization_hint: None,
            size_bytes: Some(100),
            sha256: None,
            shards: None,
        })
    }

    fn start_catalog() -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
        start_with_catalog(
            &tokio::runtime::Handle::current(),
            snapshot,
            |_| Err("unused".into()),
            || async { Err("unused".into()) },
            |_| async { Err("unused".into()) },
            |_, _| async { unreachable!("no benchmark in catalog tests") },
            || Ok(Vec::new()),
            Arc::new(FakeCatalog),
        )
    }

    #[tokio::test]
    async fn catalog_commands_return_live_cached_and_failed_results() {
        let (commands, events) = start_catalog();
        commands
            .send(CoreCommand::SearchCatalog {
                id: 1,
                query: "qwen".into(),
                cursor: None,
            })
            .await
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::CatalogLoaded { id: 1, result: Ok(Some(load)) } if load.page.repos[0].repo_id == "o/qwen"));
        commands
            .send(CoreCommand::LoadCachedCatalog {
                id: 2,
                query: "qwen".into(),
            })
            .await
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::CatalogLoaded { id: 2, result: Ok(Some(load)) } if matches!(load.source, workstation_runtimes::catalog_service::LoadSource::Cached { fetched_at_unix_ms: 7 })));
        commands
            .send(CoreCommand::LoadCachedCatalog {
                id: 3,
                query: "empty".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::CatalogLoaded {
                id: 3,
                result: Ok(None)
            }
        ));
        commands
            .send(CoreCommand::SearchCatalog {
                id: 4,
                query: "fail".into(),
                cursor: None,
            })
            .await
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::CatalogLoaded { id: 4, result: Err(message) } if message == "fixture failure"));
        commands
            .send(CoreCommand::ExpandRelease {
                id: 5,
                repo_id: "o/n".into(),
                pinned: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::ReleaseLoaded {
                id: 5,
                result: Err(_)
            }
        ));
    }

    #[tokio::test]
    async fn a_newer_search_aborts_the_stuck_older_one() {
        let (commands, events) = start_catalog();
        commands
            .send(CoreCommand::SearchCatalog {
                id: 1,
                query: "slow".into(),
                cursor: None,
            })
            .await
            .unwrap();
        commands
            .send(CoreCommand::SearchCatalog {
                id: 2,
                query: "next".into(),
                cursor: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::CatalogLoaded {
                id: 2,
                result: Ok(Some(_))
            }
        ));
        // The aborted request never reports; other commands still flow.
        commands
            .send(CoreCommand::ExpandRelease {
                id: 3,
                repo_id: "o/n".into(),
                pinned: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::ReleaseLoaded { id: 3, .. }
        ));
    }

    #[tokio::test]
    async fn a_stuck_search_does_not_block_other_work() {
        let (commands, events) = start_catalog();
        commands
            .send(CoreCommand::SearchCatalog {
                id: 1,
                query: "slow".into(),
                cursor: None,
            })
            .await
            .unwrap();
        commands
            .send(CoreCommand::ExpandRelease {
                id: 2,
                repo_id: "o/n".into(),
                pinned: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::ReleaseLoaded { id: 2, .. }
        ));
    }

    #[tokio::test]
    async fn a_catalog_panic_becomes_a_failure_for_that_request() {
        let (commands, events) = start_catalog();
        commands
            .send(CoreCommand::SearchCatalog {
                id: 9,
                query: "panic".into(),
                cursor: None,
            })
            .await
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::CatalogLoaded { id: 9, result: Err(message) } if message.contains("Catalog worker failed")));
    }

    #[tokio::test]
    async fn inspection_returns_details_failures_and_attributes_panics() {
        let (commands, events) = start_catalog();
        for (id, name) in [(1, "ok.gguf"), (2, "bad.gguf"), (3, "panic.gguf")] {
            commands
                .send(CoreCommand::InspectArtifact {
                    id,
                    artifact: artifact_named(name),
                })
                .await
                .unwrap();
            let event = receive(&events).await;
            match (id, event) {
                (
                    1,
                    UiEvent::ArtifactInspected {
                        id: 1,
                        result: Ok(details),
                    },
                ) => {
                    assert_eq!(
                        details.report.metadata.architecture.as_deref(),
                        Some("qwen2")
                    );
                    assert_eq!(details.source, LoadSource::Live);
                }
                (
                    2,
                    UiEvent::ArtifactInspected {
                        id: 2,
                        result: Err(message),
                    },
                ) => {
                    assert_eq!(message, "fixture header failure");
                }
                (
                    3,
                    UiEvent::ArtifactInspected {
                        id: 3,
                        result: Err(message),
                    },
                ) => {
                    assert!(message.contains("Inspection worker failed"));
                }
                (_, other) => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_newer_inspection_aborts_a_stuck_older_one() {
        let (commands, events) = start_catalog();
        for (id, name) in [(1, "slow.gguf"), (2, "ok.gguf")] {
            commands
                .send(CoreCommand::InspectArtifact {
                    id,
                    artifact: artifact_named(name),
                })
                .await
                .unwrap();
        }
        assert!(matches!(
            receive(&events).await,
            UiEvent::ArtifactInspected {
                id: 2,
                result: Ok(_)
            }
        ));
    }

    #[tokio::test]
    async fn a_saved_page_read_does_not_cancel_a_live_search() {
        let (commands, events) = start_catalog();
        commands
            .send(CoreCommand::SearchCatalog {
                id: 1,
                query: "fail".into(),
                cursor: None,
            })
            .await
            .unwrap();
        commands
            .send(CoreCommand::LoadCachedCatalog {
                id: 2,
                query: "".into(),
            })
            .await
            .unwrap();
        let mut seen = vec![];
        for _ in 0..2 {
            if let UiEvent::CatalogLoaded { id, .. } = receive(&events).await {
                seen.push(id);
            }
        }
        seen.sort();
        assert_eq!(seen, [1, 2]);
    }

    fn fit_request(id: u64) -> CoreCommand {
        use workstation_core::fit::{gguf_fit::LayoutAssumption, memory_math::KvCacheType};
        CoreCommand::EstimateFit(FitRequest {
            id,
            path: "unused-fixture-path".into(),
            scenario: crate::fit_workflow::parse_scenario(
                "4096",
                "512",
                KvCacheType::F16,
                KvCacheType::F16,
            )
            .unwrap(),
            layout: LayoutAssumption::Unconfirmed,
        })
    }

    #[tokio::test]
    async fn hardware_coalescing_preserves_a_queued_fit_request() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_workers(
            &tokio::runtime::Handle::current(),
            move || {
                started.send(()).unwrap();
                gate.lock().unwrap().recv().unwrap();
                snapshot()
            },
            |_| Err("Fixture inspection failure".into()),
        );
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        commands.try_send(fit_request(42)).unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::HardwareDetected(_)
        ));
        assert!(matches!(receive(&events).await,
            UiEvent::FitFinished { id: 42, result: Err(message) } if message == "Fixture inspection failure"));
    }

    #[tokio::test]
    async fn fit_jobs_are_not_coalesced_and_do_not_block_the_async_executor() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_workers(
            &tokio::runtime::Handle::current(),
            snapshot,
            move |request| {
                started.send(request.id).unwrap();
                gate.lock().unwrap().recv().unwrap();
                Err("Fixture fit result".into())
            },
        );
        commands.try_send(fit_request(1)).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
                .await
                .unwrap(),
            Some(1)
        );
        commands.try_send(fit_request(2)).unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::FitFinished { id: 1, .. }
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
                .await
                .unwrap(),
            Some(2)
        );
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::FitFinished { id: 2, .. }
        ));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn closing_window_releases_commands_during_fit_and_panics_are_reported() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) =
            start_with_workers(&tokio::runtime::Handle::current(), snapshot, move |_| {
                started.send(()).unwrap();
                let _ = gate.lock().unwrap().recv();
                Err("Fixture ended".into())
            });
        commands.try_send(fit_request(9)).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        events.close();
        let closed = tokio::time::timeout(Duration::from_secs(2), commands.closed()).await;
        release.send(()).unwrap();
        assert!(closed.is_ok());

        let (commands, events) =
            start_with_workers(&tokio::runtime::Handle::current(), snapshot, |_| {
                panic!("Fixture inspection panic")
            });
        commands.try_send(fit_request(10)).unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::FitFinished { id: 10, result: Err(message) } if message.contains("Model inspection worker failed")));
    }
    #[tokio::test]
    async fn preserves_partial_runtime_results_and_inspection_request_identity() {
        let (commands, events) = start_with_runtime_workers(
            &tokio::runtime::Handle::current(),
            snapshot,
            |_| Err("unused fit".into()),
            || async {
                Ok(Discovery {
                    version: "fixture".into(),
                    installed: Ok(vec![]),
                    running: Err("fixture inventory failure".into()),
                })
            },
            |model| async move {
                if model == "missing" {
                    return Err("fixture model removed".into());
                }
                assert_eq!(model, "chosen:latest");
                Ok(ModelInspection {
                    metadata: Err("fixture metadata failure".into()),
                    running: Ok(vec![]),
                })
            },
        );
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::OllamaDiscovered(snapshot) if snapshot.installed.is_ok() && snapshot.running.is_err()));
        commands
            .try_send(CoreCommand::InspectOllama {
                id: 17,
                model: "chosen:latest".into(),
            })
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::OllamaInspected { id: 17, result: Ok(report) }
                if report.metadata.is_err() && report.running.is_ok()));
        commands
            .try_send(CoreCommand::InspectOllama {
                id: 18,
                model: "missing".into(),
            })
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::OllamaInspected { id: 18, result: Err(message) } if message == "fixture model removed"));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn closing_window_cancels_pending_discovery_and_model_inspection() {
        for inspection in [false, true] {
            let (started, mut started_rx) = mpsc::unbounded_channel();
            let inspection_started = started.clone();
            let (commands, events) = start_with_runtime_workers(
                &tokio::runtime::Handle::current(),
                snapshot,
                |_| Err("unused fit".into()),
                move || {
                    started.send(()).unwrap();
                    std::future::pending()
                },
                move |_| {
                    inspection_started.send(()).unwrap();
                    std::future::pending()
                },
            );
            commands
                .try_send(if inspection {
                    CoreCommand::InspectOllama {
                        id: 1,
                        model: "fixture".into(),
                    }
                } else {
                    CoreCommand::DiscoverOllama
                })
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
                .await
                .unwrap()
                .unwrap();
            events.close();
            tokio::time::timeout(Duration::from_secs(2), commands.closed())
                .await
                .unwrap();
        }
    }
    fn benchmark_model() -> InstalledModel {
        InstalledModel {
            name: "fixture".into(),
            digest: None,
            size: None,
            details: None,
            remote_model: None,
            remote_host: None,
        }
    }

    fn with_benchmark<B>(
        run: impl Fn(InstalledModel, tokio::sync::watch::Receiver<bool>) -> B + Send + 'static,
    ) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>)
    where
        B: std::future::Future<Output = BenchmarkOutcome> + Send + 'static,
    {
        start_with_all_workers(
            &tokio::runtime::Handle::current(),
            snapshot,
            |_| Err("unused fit".into()),
            || async { Err("fixture discovery failure".into()) },
            |_| async { Err("unused inspection".into()) },
            move |model, cancel| {
                let future = run(model, cancel);
                async move {
                    PersistedRun {
                        outcome: future.await,
                        saved: Ok(1),
                        resources: Some(ResourceSummary {
                            samples: 2,
                            ..ResourceSummary::default()
                        }),
                    }
                }
            },
            || Ok(vec![]),
        )
    }

    #[tokio::test]
    async fn benchmark_does_not_block_other_commands_and_duplicate_runs_are_rejected() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (commands, events) = with_benchmark(move |_, mut cancel| {
            started.send(()).unwrap();
            async move {
                cancel.changed().await.unwrap();
                BenchmarkOutcome::Cancelled(GenerationProgress::default())
            }
        });
        let (cancel, cancellation) = tokio::sync::watch::channel(false);
        commands
            .try_send(CoreCommand::RunQuickBenchmark {
                id: 71,
                model: benchmark_model(),
                cancel: cancellation,
            })
            .unwrap();
        started_rx.recv().await.unwrap();
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        assert!(matches!(receive(&events).await, UiEvent::OllamaFailed(_)));
        let (_other, cancellation) = tokio::sync::watch::channel(false);
        commands
            .try_send(CoreCommand::RunQuickBenchmark {
                id: 72,
                model: benchmark_model(),
                cancel: cancellation,
            })
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::BenchmarkFinished {
                id: 72,
                outcome: BenchmarkOutcome::Failed { .. },
                ..
            }
        ));
        cancel.send(true).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::BenchmarkFinished {
                id: 71,
                outcome: BenchmarkOutcome::Cancelled(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn history_is_loaded_by_request_and_result_precedes_save_feedback() {
        let (commands, events) = with_benchmark(|_, _| async {
            BenchmarkOutcome::Cancelled(GenerationProgress::default())
        });
        commands
            .send(CoreCommand::LoadBenchmarkHistory { id: 10 })
            .await
            .unwrap();
        assert!(
            matches!(receive(&events).await, UiEvent::BenchmarkHistoryLoaded { id: 10, result: Ok(rows) } if rows.is_empty())
        );
        let (_cancel, cancel) = tokio::sync::watch::channel(false);
        commands
            .send(CoreCommand::RunQuickBenchmark {
                id: 11,
                model: benchmark_model(),
                cancel,
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::BenchmarkFinished { id: 11, resources: Some(readings), .. } if readings.samples == 2
        ));
        assert!(matches!(
            receive(&events).await,
            UiEvent::BenchmarkHistorySaved {
                benchmark_id: 11,
                result: Ok(1)
            }
        ));
    }

    #[tokio::test]
    async fn benchmark_worker_panics_become_retryable_failures() {
        let (commands, events) = with_benchmark(|_, _| async { panic!("fixture benchmark panic") });
        for id in [81, 82] {
            let (_cancel, cancellation) = tokio::sync::watch::channel(false);
            commands
                .try_send(CoreCommand::RunQuickBenchmark {
                    id,
                    model: benchmark_model(),
                    cancel: cancellation,
                })
                .unwrap();
            assert!(
                matches!(receive(&events).await, UiEvent::BenchmarkFinished { id: reply_id, outcome: BenchmarkOutcome::Failed { .. }, .. } if reply_id == id)
            );
            assert!(
                matches!(receive(&events).await, UiEvent::BenchmarkHistorySaved { benchmark_id, result: Err(_) } if benchmark_id == id)
            );
        }
    }

    #[tokio::test]
    async fn closing_window_drops_active_benchmark_future() {
        struct Dropped(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (dropped, mut dropped_rx) = mpsc::unbounded_channel();
        let (commands, events) = with_benchmark(move |_, _| {
            let started = started.clone();
            let dropped = dropped.clone();
            async move {
                let (sender, receiver) = tokio::sync::oneshot::channel();
                dropped.send(receiver).unwrap();
                let _guard = Dropped(Some(sender));
                started.send(()).unwrap();
                std::future::pending().await
            }
        });
        let (_cancel, cancellation) = tokio::sync::watch::channel(false);
        commands
            .try_send(CoreCommand::RunQuickBenchmark {
                id: 91,
                model: benchmark_model(),
                cancel: cancellation,
            })
            .unwrap();
        started_rx.recv().await.unwrap();
        let dropped = dropped_rx.recv().await.unwrap();
        events.close();
        tokio::time::timeout(Duration::from_secs(2), dropped)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), commands.closed())
            .await
            .unwrap();
    }
}
