// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in live M2 acceptance run. It talks to api.github.com / github.com and runs the
//! installed `llama-server`. Tests never do either; this is run by hand to record evidence.
//!
//! ```text
//! cargo run --locked -p workstation-runtimes --example m2_acceptance -- <empty-work-dir> install [cpu|vulkan|cuda]
//! ```
//!
//! Everything lives under the work directory; the application's own data is not touched.

use std::{path::PathBuf, sync::atomic::AtomicBool};
use workstation_runtimes::llamacpp::{
    install::Installer,
    release::{Backend, detect_backends},
};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let work = PathBuf::from(args.next().expect("work directory"));
    let step = args.next().unwrap_or_default();
    let backend = args
        .next()
        .and_then(|b| Backend::parse(&b))
        .unwrap_or(Backend::Cpu);
    if !work.is_absolute() {
        panic!("work directory must be absolute");
    }
    match step.as_str() {
        "install" => install(&work, backend).await,
        other => panic!("unknown step {other:?}"),
    }
}

async fn install(work: &std::path::Path, backend: Backend) {
    println!("backends offered on this machine: {:?}", detect_backends());
    let installer = Installer::new().expect("client");
    let found = installer
        .list_candidates(&[backend])
        .await
        .expect("release listing");
    let candidate = found.first().expect("no verified build for that backend");
    println!(
        "selected {} {} ({} archive(s), {} bytes)",
        candidate.tag,
        backend.as_str(),
        candidate.archives.len(),
        candidate.total_size()
    );
    let root = work.join("llamacpp");
    let cancel = AtomicBool::new(false);
    let installed = installer
        .install(&root, candidate, &cancel, |p| {
            println!(
                "  archive {}/{}: {}/{} bytes",
                p.archive + 1,
                p.archives,
                p.done,
                p.total
            );
        })
        .await
        .expect("install");
    println!(
        "installed {} sha256 {}",
        installed.dir_name, installed.sha256
    );
    println!(
        "server: {}",
        root.join(&installed.dir_name)
            .join("llama-server")
            .display()
    );
}
