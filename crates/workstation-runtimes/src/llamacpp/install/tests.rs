// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::{
    llamacpp::extract_tests::{Dir, Item, tgz},
    test_server::{Reply, Server, json_reply, route, status},
};
use serde_json::json;
use std::sync::atomic::AtomicBool;

fn sha(bytes: &[u8]) -> String {
    finish_hex(Sha256::new().chain_update(bytes))
}

fn good_archive(script: &'static [u8]) -> Vec<u8> {
    tgz(&[
        Item::Dir("llama-b1/"),
        Item::File("llama-b1/llama-server", script.to_vec(), 0o755),
        Item::File("llama-b1/libx.so.0.1", b"lib".to_vec(), 0o755),
        Item::Link("llama-b1/libx.so", "libx.so.0.1"),
    ])
}

fn candidate(archive: &[u8]) -> Candidate {
    Candidate {
        tag: "b1".into(),
        backend: Backend::Cpu,
        archives: vec![Asset {
            name: "llama-b1-bin-ubuntu-x64.tar.gz".into(),
            size: archive.len() as u64,
            sha256: sha(archive),
        }],
    }
}

const PATH: &str = "GET /dl/b1/llama-b1-bin-ubuntu-x64.tar.gz HTTP/1.1";

fn bytes(body: &[u8]) -> Reply {
    Reply::Bytes {
        code: 200,
        body: body.to_vec(),
        headers: Vec::new(),
    }
}

fn leftovers(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".lock")
        .collect();
    names.sort();
    names
}

async fn install(
    server: &Server,
    root: &Path,
    candidate: &Candidate,
    cancel: &AtomicBool,
) -> Result<Installed, InstallError> {
    Installer::for_test(server.port())
        .install(root, candidate, cancel, |_| {})
        .await
}

#[tokio::test]
async fn a_verified_build_is_published_and_a_second_install_is_refused() {
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let cancel = AtomicBool::new(false);
    let done = install(&server, &root, &candidate(&archive), &cancel)
        .await
        .unwrap();
    assert_eq!(done.dir_name, "b1-cpu");
    assert_eq!(leftovers(&root), ["b1-cpu"]);
    assert!(root.join("b1-cpu/llama-server").is_file());
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        install(&server, &root, &candidate(&archive), &cancel).await,
        Err(InstallError::AlreadyInstalled)
    );
    assert_eq!(server.lines().len(), 1, "no second download");
}

#[tokio::test]
async fn a_checksum_mismatch_publishes_nothing_and_leaves_no_staging() {
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    let mut wrong = candidate(&archive);
    wrong.archives[0].sha256 = "0".repeat(64);
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let result = install(&server, &root, &wrong, &AtomicBool::new(false)).await;
    assert_eq!(result, Err(InstallError::Integrity));
    assert!(leftovers(&root).is_empty());
}

#[tokio::test]
async fn size_lies_and_overlong_bodies_are_integrity_failures() {
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    let mut short = candidate(&archive);
    short.archives[0].size += 10;
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    assert_eq!(
        install(&server, &root, &short, &AtomicBool::new(false)).await,
        Err(InstallError::Integrity)
    );
    assert!(leftovers(&root).is_empty());
}

#[tokio::test]
async fn a_build_that_will_not_run_is_not_published() {
    for script in [&b"#!/bin/sh\nexit 3\n"[..], &b"not a program"[..]] {
        let archive = tgz(&[Item::File("llama-b1/llama-server", script.to_vec(), 0o755)]);
        let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
        let dir = Dir::new();
        let root = dir.0.join("llamacpp");
        let result = install(
            &server,
            &root,
            &candidate(&archive),
            &AtomicBool::new(false),
        )
        .await;
        assert_eq!(result, Err(InstallError::WontRun));
        assert!(leftovers(&root).is_empty());
    }
    // And an archive with no server at all.
    let archive = tgz(&[Item::File("llama-b1/readme", b"x".to_vec(), 0o644)]);
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let result = install(
        &server,
        &root,
        &candidate(&archive),
        &AtomicBool::new(false),
    )
    .await;
    assert_eq!(result, Err(InstallError::WontRun));
}

#[tokio::test]
async fn hostile_archives_are_refused_before_anything_runs() {
    let archive = tgz(&[
        Item::File(
            "llama-b1/llama-server",
            b"#!/bin/sh\nexit 0\n".to_vec(),
            0o755,
        ),
        Item::File("llama-b1/../escape", b"x".to_vec(), 0o644),
    ]);
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let result = install(
        &server,
        &root,
        &candidate(&archive),
        &AtomicBool::new(false),
    )
    .await;
    assert!(matches!(
        result,
        Err(InstallError::Archive(ExtractError::Unsafe(_)))
    ));
    assert!(leftovers(&root).is_empty());
    assert!(!dir.0.join("escape").exists());
}

#[tokio::test]
async fn redirects_stay_on_the_allowed_host() {
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    // A second server stands in for a foreign host.
    let foreign = Server::start(|_| vec![route("GET /x HTTP/1.1", bytes(&archive))]);
    let foreign_port = foreign.port();
    let server = Server::start(|_| {
        vec![
            route(PATH, Reply::Redirect("/dl/real".into())),
            route("GET /dl/real HTTP/1.1", bytes(&archive)),
            route(
                "GET /dl/b2/llama-b2-bin-ubuntu-x64.tar.gz HTTP/1.1",
                Reply::Redirect(format!("http://127.0.0.1:{foreign_port}/x")),
            ),
        ]
    });
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let cancel = AtomicBool::new(false);
    install(&server, &root, &candidate(&archive), &cancel)
        .await
        .unwrap();

    let mut other = candidate(&archive);
    other.tag = "b2".into();
    other.archives[0].name = "llama-b2-bin-ubuntu-x64.tar.gz".into();
    let result = install(&server, &root, &other, &cancel).await;
    assert!(matches!(result, Err(InstallError::Unsafe(_))));
    assert!(
        foreign.lines().is_empty(),
        "the foreign host was never contacted"
    );
}

#[tokio::test]
async fn cancel_stops_and_cleans_up() {
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let result = install(&server, &root, &candidate(&archive), &AtomicBool::new(true)).await;
    assert_eq!(result, Err(InstallError::Cancelled));
    assert!(leftovers(&root).is_empty());
}

#[tokio::test]
async fn http_errors_are_typed() {
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    for (code, expected) in [
        (404, InstallError::BadResponse),
        (403, InstallError::RateLimited),
    ] {
        let server = Server::start(|_| vec![route(PATH, status(code))]);
        let result = install(
            &server,
            &root,
            &candidate(&archive),
            &AtomicBool::new(false),
        )
        .await;
        assert_eq!(result, Err(expected));
    }
}

#[tokio::test]
async fn crash_leftovers_are_cleaned_and_a_second_installer_is_locked_out() {
    let archive = good_archive(b"#!/bin/sh\nexit 0\n");
    let server = Server::start(|_| vec![route(PATH, bytes(&archive))]);
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    prepare_root(&root).unwrap();
    fs::create_dir(root.join(".stage-dead")).unwrap();
    fs::write(root.join(".stage-dead/junk"), b"x").unwrap();
    {
        let _held = RootLock::acquire(&root).unwrap();
        let result = install(
            &server,
            &root,
            &candidate(&archive),
            &AtomicBool::new(false),
        )
        .await;
        assert_eq!(result, Err(InstallError::Busy));
        assert!(
            root.join(".stage-dead").exists(),
            "not touched while locked"
        );
    }
    install(
        &server,
        &root,
        &candidate(&archive),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    assert_eq!(leftovers(&root), ["b1-cpu"]);
}

#[test]
fn removal_refuses_the_running_build_links_and_bad_names() {
    let dir = Dir::new();
    let root = dir.0.join("llamacpp");
    prepare_root(&root).unwrap();
    fs::create_dir(root.join("b1-cpu")).unwrap();
    fs::write(root.join("b1-cpu/f"), b"x").unwrap();
    assert!(remove_install(&root, "b1-cpu", Some("b1-cpu")).is_err());
    assert!(root.join("b1-cpu").exists());
    for bad in ["../x", ".stage-1", "a/b", ""] {
        assert!(remove_install(&root, bad, None).is_err(), "{bad}");
    }
    let outside = dir.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), b"x").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("b2-cpu")).unwrap();
    assert!(remove_install(&root, "b2-cpu", None).is_err());
    assert!(outside.join("keep").exists());
    remove_install(&root, "b1-cpu", Some("other")).unwrap();
    assert!(!root.join("b1-cpu").exists());
    remove_install(&root, "b1-cpu", None).unwrap();
}

#[test]
fn a_root_that_is_a_link_or_open_to_others_is_refused() {
    let dir = Dir::new();
    let real = dir.0.join("real");
    fs::create_dir(&real).unwrap();
    let link = dir.0.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(prepare_root(&link), Err(InstallError::Unsafe(_))));
    let open = dir.0.join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(prepare_root(&open), Err(InstallError::Unsafe(_))));
}

#[tokio::test]
async fn the_release_listing_is_requested_once_and_bounded() {
    let body = json!([{
        "tag_name": "b1", "draft": false,
        "assets": [{
            "name": "llama-b1-bin-ubuntu-x64.tar.gz", "size": 10, "state": "uploaded",
            "digest": format!("sha256:{}", "a".repeat(64)),
        }]
    }]);
    let server = Server::start(|_| {
        vec![route(
            "GET /repos/ggml-org/llama.cpp/releases?per_page=20 HTTP/1.1",
            json_reply(body),
        )]
    });
    let found = Installer::for_test(server.port())
        .list_candidates(&[Backend::Cpu, Backend::Vulkan])
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].backend, Backend::Cpu);
    assert_eq!(server.lines().len(), 1);

    let big = Server::start(|_| {
        vec![route(
            "GET /repos/ggml-org/llama.cpp/releases?per_page=20 HTTP/1.1",
            Reply::StreamedOversize,
        )]
    });
    let result = Installer::for_test(big.port())
        .list_candidates(&[Backend::Cpu])
        .await;
    assert!(result.is_err());
}
