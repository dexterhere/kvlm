// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK coverage; catalog data is supplied as typed fixtures.

use super::*;
use workstation_core::models::{
    artifact::{ArtifactFormat, ModelArtifact},
    catalog::{CatalogPage, RepoSummary},
    family::{FamilySource, ModelFamily},
    release::{GatedAccess, ModelRelease},
};

const REV: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn page(next: Option<&str>) -> Box<CatalogLoad> {
    Box::new(CatalogLoad {
        page: CatalogPage {
            repos: vec![RepoSummary {
                repo_id: "<b>o</b>/n-GGUF".into(),
                downloads: Some(5),
                likes: None,
                gated: GatedAccess::Manual,
            }],
            next_cursor: next.map(str::to_owned),
        },
        source: LoadSource::Cached {
            fetched_at_unix_ms: 1,
        },
        notice: None,
    })
}

fn release() -> Box<ReleaseLoad> {
    Box::new(ReleaseLoad {
        release: ModelRelease {
            repo_id: "<b>o</b>/n-GGUF".into(),
            revision: REV.into(),
            family: ModelFamily {
                name: "n".into(),
                publisher: None,
                source: FamilySource::RepoName,
            },
            license: None,
            gated: GatedAccess::Manual,
            artifacts: vec![ModelArtifact {
                artifact_id: "hf:fixture".into(),
                repo_id: "<b>o</b>/n-GGUF".into(),
                revision: REV.into(),
                filename: "m-Q4_K_M.gguf".into(),
                format: ArtifactFormat::Gguf,
                quantization_hint: Some("Q4_K_M".into()),
                size_bytes: Some(4_683_073_536),
                sha256: None,
                shards: None,
            }],
            dropped_invalid: 0,
            other_weight_files: 2,
        },
        source: LoadSource::Live,
        notice: None,
    })
}

fn details() -> Box<crate::bridge::ArtifactDetails> {
    use workstation_core::{
        fit::{gguf_fit::LayoutAssumption, memory_math::KvCacheType},
        hardware::{HardwareProfile, MemoryPool},
        models::gguf::GgufMetadata,
    };
    let metadata = GgufMetadata {
        file_bytes: 100,
        tensor_count: 1,
        tensor_bytes: 4,
        alignment: 32,
        architecture: Some("future-net".into()),
        attention: None,
        warnings: vec![],
    };
    let scenario =
        crate::fit_workflow::parse_scenario("8192", "512", KvCacheType::F16, KvCacheType::F16)
            .unwrap();
    let hardware = HardwareProfile {
        profile_hash: None,
        cpu: None,
        system_ram: Some(MemoryPool {
            total_bytes: 8 << 30,
            available_bytes: Some(4 << 30),
        }),
        gpus: vec![],
        issues: vec![],
        captured_at_unix_ms: 1,
    };
    let report = crate::fit_workflow::fit_report(
        metadata,
        &scenario,
        LayoutAssumption::UniformFullAttention,
        hardware,
    )
    .unwrap();
    Box::new(crate::bridge::ArtifactDetails {
        report,
        source: LoadSource::Live,
        notice: None,
    })
}

fn pump(commands: &mut mpsc::Receiver<CoreCommand>) -> CoreCommand {
    let ctx = gtk::glib::MainContext::default();
    for _ in 0..200 {
        while ctx.iteration(false) {}
        if let Ok(command) = commands.try_recv() {
            return command;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("no command was sent");
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn saved_page_search_expand_and_stale_answers() {
    adw::init().expect("GTK display is available");
    let (commands, mut rx) = mpsc::channel(4);
    let explorer = Explorer::new(commands);
    gtk::Window::builder()
        .default_width(360)
        .child(&explorer.widget)
        .build()
        .present();

    explorer.load_saved();
    let CoreCommand::LoadCachedCatalog { id, .. } = pump(&mut rx) else {
        panic!("saved read first")
    };
    explorer.receive(&UiEvent::CatalogLoaded {
        id,
        result: Ok(Some(page(Some("c")))),
    });
    assert!(explorer.more.is_visible());
    assert!(explorer.status.text().contains("saved"));

    // An answer for an older request is ignored.
    explorer.receive(&UiEvent::CatalogLoaded {
        id: id + 99,
        result: Err("stale".into()),
    });
    assert!(explorer.status.text().contains("saved"));

    // Expanding sends exactly one request; the answer lands under the literal title.
    let row = explorer.state.borrow().rows["<b>o</b>/n-GGUF"].row.clone();
    row.set_expanded(true);
    let CoreCommand::ExpandRelease {
        id,
        repo_id,
        pinned: None,
    } = pump(&mut rx)
    else {
        panic!("expand")
    };
    assert_eq!(repo_id, "<b>o</b>/n-GGUF");
    assert_eq!(row.title(), "<b>o</b>/n-GGUF");
    explorer.receive(&UiEvent::ReleaseLoaded {
        id,
        result: Ok(release()),
    });
    assert!(explorer.state.borrow().rows[&repo_id].loaded);

    // Expanding a file reads its header once; the answer shows the conditional fit.
    let artifact_row = explorer.state.borrow().artifacts["hf:fixture"].row.clone();
    artifact_row.set_expanded(true);
    let CoreCommand::InspectArtifact { id, artifact } = pump(&mut rx) else {
        panic!("inspect")
    };
    assert_eq!(artifact.filename, "m-Q4_K_M.gguf");
    explorer.receive(&UiEvent::ArtifactInspected {
        id: id + 7,
        result: Err("stale".into()),
    });
    assert!(explorer.state.borrow().artifacts["hf:fixture"].loading);
    explorer.receive(&UiEvent::ArtifactInspected {
        id,
        result: Ok(details()),
    });
    {
        let state = explorer.state.borrow();
        let entry = &state.artifacts["hf:fixture"];
        assert!(entry.loaded && !entry.loading);
        assert!(entry.children.len() >= 4);
    }
    artifact_row.set_expanded(false);
    artifact_row.set_expanded(true);
    // Expanding again asks again: the header is saved, but memory budgets are fresh.
    let CoreCommand::InspectArtifact { id, .. } = pump(&mut rx) else {
        panic!("re-inspect")
    };
    explorer.receive(&UiEvent::ArtifactInspected {
        id,
        result: Ok(details()),
    });

    // A failed search keeps the list and offers a retry.
    explorer.entry.set_text("qwen");
    explorer.entry.emit_activate();
    let CoreCommand::SearchCatalog {
        id,
        query,
        cursor: None,
    } = pump(&mut rx)
    else {
        panic!("search")
    };
    assert_eq!(query, "qwen");
    explorer.receive(&UiEvent::CatalogLoaded {
        id,
        result: Err("offline".into()),
    });
    assert!(explorer.status.text().contains("Press Search to retry"));
    assert_eq!(explorer.state.borrow().rows.len(), 1);
    // The failed query is not adopted: the next page still continues the shown results.
    assert_eq!(explorer.state.borrow().query, "");
    assert!(explorer.more.is_sensitive());
}
