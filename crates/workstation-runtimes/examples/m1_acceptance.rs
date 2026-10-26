// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in live M1 acceptance run. It talks to huggingface.co and, for the keyring steps, to
//! the desktop Secret Service. Tests never do either; this is run by hand to record evidence.
//!
//! ```text
//! cargo run --locked -p workstation-runtimes --example m1_acceptance -- <empty-work-dir> catalog
//! cargo run --locked -p workstation-runtimes --example m1_acceptance -- <empty-work-dir> gated
//! cargo run --locked -p workstation-runtimes --example m1_acceptance -- <empty-work-dir> keyring-save
//! cargo run --locked -p workstation-runtimes --example m1_acceptance -- <empty-work-dir> keyring-check
//! ```
//!
//! `catalog` uses only the work directory (its own database and model store). The keyring
//! steps refuse to touch an existing saved token. No token or file content is printed.

use std::{path::PathBuf, sync::Arc, sync::atomic::AtomicBool};
use tokio::sync::watch;
use workstation_core::models::{
    artifact::ModelArtifact,
    dedup::{DedupRoot, Limits, RootKind, Wanted, scan},
    gguf::inspect_prefix,
    release::GatedAccess,
};
use workstation_runtimes::{
    downloader::{DownloadEnd, Downloader, Stop},
    huggingface::{HfError, HuggingFaceClient},
    secrets::{Token, TokenService, TokenSlot},
};

const MIN: u64 = 1 << 20;
const MAX: u64 = 300 << 20;

fn smallest_usable(artifacts: &[ModelArtifact]) -> Option<&ModelArtifact> {
    artifacts
        .iter()
        .filter(|a| a.shards.is_none() && a.sha256.is_some())
        .filter(|a| a.size_bytes.is_some_and(|s| (MIN..=MAX).contains(&s)))
        .min_by_key(|a| a.size_bytes)
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().ok_or("work directory missing")?);
    match args.next().as_deref() {
        Some("catalog") => catalog(dir).await,
        Some("gated") => {
            let client = HuggingFaceClient::new(TokenSlot::default()).map_err(|e| e.to_string())?;
            gated(&client).await
        }
        Some("keyring-save") => keyring_save().await,
        Some("keyring-check") => keyring_check().await,
        _ => Err("mode: catalog | gated | keyring-save | keyring-check".into()),
    }
}

async fn catalog(dir: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let client = Arc::new(HuggingFaceClient::new(TokenSlot::default()).map_err(|e| e.to_string())?);

    let page = client
        .search("tiny", None)
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "search: {} repositories, first {:?}",
        page.repos.len(),
        page.repos.first().map(|r| &r.repo_id)
    );

    // Details: the smallest checksummed single file among the first results.
    let mut chosen = None;
    for repo in page.repos.iter().take(12) {
        let Ok(release) = client.release(&repo.repo_id, None).await else {
            continue;
        };
        if let Some(artifact) = smallest_usable(&release.artifacts) {
            println!(
                "details: {} @ {} -> {} ({} bytes, sha256 listed, license {:?})",
                repo.repo_id,
                &release.revision[..12],
                artifact.filename,
                artifact.size_bytes.unwrap_or(0),
                release.license
            );
            chosen = Some(artifact.clone());
            break;
        }
    }
    let artifact = chosen.ok_or("no small checksummed GGUF found in the first results")?;
    let size = artifact.size_bytes.unwrap_or(0);

    // Header and fit input: the first bytes only.
    let prefix = client
        .fetch_prefix(
            &artifact.repo_id,
            &artifact.revision,
            &artifact.filename,
            size,
        )
        .await
        .map_err(|e| e.to_string())?;
    match inspect_prefix(&prefix, size) {
        Ok(meta) => println!(
            "header: arch {:?}, {} tensors, weights {} bytes",
            meta.architecture, meta.tensor_count, meta.tensor_bytes
        ),
        Err(error) => {
            println!("header: not usable ({error:?}); download still verified by checksum")
        }
    }

    // Interrupted-then-resumed download into an isolated store.
    let models = dir.join("models");
    let downloader = Downloader::with_parts(client.clone(), dir.join("history.db"), models.clone());
    let (stop_tx, stop_rx) = watch::channel(None);
    let half = size / 4;
    let first = downloader
        .run(artifact.clone(), stop_rx, move |p| {
            if p.bytes_done >= half {
                let _ = stop_tx.send(Some(Stop::Pause));
            }
        })
        .await;
    println!(
        "first run: {:?} at {} of {} bytes",
        first.end, first.bytes_done, size
    );
    let part_before = std::fs::read_dir(models.join("partial"))
        .map(|d| d.count())
        .unwrap_or(0);
    println!("partial files kept after pause: {part_before}");
    let (_keep, resume_rx) = watch::channel(None);
    let second = downloader.run(artifact.clone(), resume_rx, |_| {}).await;
    println!("resume run: {:?}, {} bytes", second.end, second.bytes_done);
    if second.end != DownloadEnd::Completed {
        return Err("resume did not complete".into());
    }
    let blobs = models.join("blobs");
    println!("blob: sha256-{}", artifact.sha256.as_deref().unwrap_or(""));
    println!(
        "blob exists: {}",
        blobs
            .join(format!(
                "sha256-{}",
                artifact.sha256.as_deref().unwrap_or("")
            ))
            .exists()
    );

    // Duplicate detection finds the verified copy.
    let wanted = [Wanted {
        artifact_id: artifact.artifact_id.clone(),
        size,
        sha256: artifact.sha256.clone().unwrap_or_default(),
    }];
    let roots = [DedupRoot {
        kind: RootKind::Application,
        path: blobs,
    }];
    let report = scan(
        &roots,
        &wanted,
        None,
        &AtomicBool::new(false),
        &Limits::default(),
    );
    println!(
        "dedup: {} verified local copy, stopped {:?}",
        report.matches.len(),
        report.stopped
    );

    gated(&client).await
}

/// A gated repository refuses an anonymous file request, with the guidance a person needs.
async fn gated(client: &HuggingFaceClient) -> Result<(), String> {
    for repo in page_with_gated(client).await? {
        let Ok(release) = client.release(&repo, None).await else {
            continue;
        };
        if release.gated == GatedAccess::Open {
            continue;
        }
        let Some(file) = release.artifacts.iter().find(|a| a.size_bytes.is_some()) else {
            continue;
        };
        let refused = client
            .fetch_prefix(
                &file.repo_id,
                &file.revision,
                &file.filename,
                file.size_bytes.unwrap_or(1),
            )
            .await;
        println!(
            "gated {} ({:?}): anonymous request -> {:?}",
            repo,
            release.gated,
            refused.as_ref().err()
        );
        if matches!(refused, Err(HfError::AccessRequired)) {
            println!(
                "gated refusal: AccessRequired. Message shown: {}",
                HfError::AccessRequired
            );
            return Ok(());
        }
    }
    println!("gated refusal: not observed (no gated GGUF repository reachable)");
    Ok(())
}

/// Candidate repositories; the search list does not say which are gated, so the caller
/// reads each repository's detail.
async fn page_with_gated(client: &HuggingFaceClient) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    for query in ["llama", "gemma", "meta", "mistral"] {
        let page = client
            .search(query, None)
            .await
            .map_err(|e| e.to_string())?;
        found.extend(page.repos.into_iter().map(|r| r.repo_id));
    }
    Ok(found)
}

/// Store a throwaway token only if none is saved, then leave it for the next process.
async fn keyring_save() -> Result<(), String> {
    let service = TokenService::keyring(TokenSlot::default());
    if service.load().await.map_err(|e| e.to_string())? {
        return Err("a token is already saved; refusing to overwrite it".into());
    }
    service
        .save(Token::parse("hf_m1AcceptanceThrowawayValue").map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;
    println!("keyring: throwaway token saved (this process now exits)");
    Ok(())
}

/// A fresh process reads it back, then removes it.
async fn keyring_check() -> Result<(), String> {
    let service = TokenService::keyring(TokenSlot::default());
    let found = service.load().await.map_err(|e| e.to_string())?;
    println!("keyring: token survived restart: {found}");
    println!("keyring: in memory after load: {}", service.slot().is_set());
    service.clear().await.map_err(|e| e.to_string())?;
    let after = service.load().await.map_err(|e| e.to_string())?;
    println!("keyring: removed, still present afterwards: {after}");
    Ok(())
}
