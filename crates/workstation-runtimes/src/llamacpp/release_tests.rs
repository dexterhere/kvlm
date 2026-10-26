// SPDX-License-Identifier: GPL-3.0-or-later

use super::release::{Backend, backends_for, nvidia_major, select, valid_tag};
use serde_json::{Value, json};

fn asset(name: &str, digest: Option<&str>) -> Value {
    let mut a = json!({"name": name, "size": 1000, "state": "uploaded"});
    if let Some(d) = digest {
        a["digest"] = d.into();
    }
    a
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

fn release(tag: &str, assets: Vec<Value>) -> Value {
    json!({"tag_name": tag, "draft": false, "assets": assets})
}

#[test]
fn the_newest_release_with_a_digest_wins_and_names_come_from_the_tag() {
    let body = json!([
        release("b3", vec![asset("llama-b3-bin-ubuntu-x64.tar.gz", None)]),
        release(
            "b2",
            vec![
                asset("llama-b2-bin-ubuntu-x64.tar.gz", Some(&digest('a'))),
                asset("llama-b2-bin-ubuntu-vulkan-x64.tar.gz", Some(&digest('b'))),
                asset(
                    "llama-b2-bin-ubuntu-cuda-12.8-x64.tar.gz",
                    Some(&digest('c'))
                ),
            ]
        ),
        release(
            "b1",
            vec![asset("llama-b1-bin-ubuntu-x64.tar.gz", Some(&digest('d')))]
        ),
    ])
    .to_string();
    let found = select(
        body.as_bytes(),
        &[Backend::Cpu, Backend::Vulkan, Backend::Cuda],
    )
    .unwrap();
    let backends: Vec<_> = found.iter().map(|c| (c.tag.as_str(), c.backend)).collect();
    // CUDA is missing its runtime bundle, so it is not offered.
    assert_eq!(backends, [("b2", Backend::Cpu), ("b2", Backend::Vulkan)]);
    assert_eq!(found[0].archives[0].sha256, "a".repeat(64));
    assert_eq!(found[0].dir_name(), "b2-cpu");
}

#[test]
fn cuda_needs_both_archives_and_reports_the_combined_size() {
    let body = json!([release(
        "b9",
        vec![
            asset(
                "llama-b9-bin-ubuntu-cuda-12.8-x64.tar.gz",
                Some(&digest('a'))
            ),
            asset(
                "cudart-llama-b9-bin-ubuntu-cuda-12.8-x64.tar.gz",
                Some(&digest('b'))
            ),
        ]
    )])
    .to_string();
    let found = select(body.as_bytes(), &[Backend::Cuda]).unwrap();
    assert_eq!(found[0].archives.len(), 2);
    assert_eq!(found[0].total_size(), 2000);
}

#[test]
fn drafts_bad_tags_bad_digests_and_unfinished_uploads_are_ignored() {
    let mut draft = release(
        "b5",
        vec![asset("llama-b5-bin-ubuntu-x64.tar.gz", Some(&digest('a')))],
    );
    draft["draft"] = true.into();
    let weak = release(
        "b4",
        vec![asset("llama-b4-bin-ubuntu-x64.tar.gz", Some("md5:abc"))],
    );
    let evil = release(
        "../x",
        vec![asset(
            "llama-../x-bin-ubuntu-x64.tar.gz",
            Some(&digest('a')),
        )],
    );
    let mut pending = asset("llama-b2-bin-ubuntu-x64.tar.gz", Some(&digest('a')));
    pending["state"] = "starter".into();
    let huge = {
        let mut a = asset("llama-b1-bin-ubuntu-x64.tar.gz", Some(&digest('a')));
        a["size"] = (2u64 << 30).into();
        a
    };
    for rel in [
        draft,
        weak,
        evil,
        release("b2", vec![pending]),
        release("b1", vec![huge]),
    ] {
        let body = json!([rel]).to_string();
        assert!(select(body.as_bytes(), &[Backend::Cpu]).unwrap().is_empty());
    }
    assert!(select(b"{not json", &[Backend::Cpu]).is_err());
    assert!(select(b"{}", &[Backend::Cpu]).is_err());
}

#[test]
fn tags_and_backend_detection() {
    assert!(valid_tag("b11905") && !valid_tag("b") && !valid_tag("11905") && !valid_tag("b1.2"));
    assert_eq!(backends_for(false, None), [Backend::Cpu]);
    assert_eq!(
        backends_for(true, Some(560)),
        [Backend::Cpu, Backend::Vulkan]
    );
    assert_eq!(backends_for(true, Some(615)).last(), Some(&Backend::Cuda));
    assert_eq!(
        nvidia_major("NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release"),
        Some(615)
    );
    assert_eq!(nvidia_major("nothing here"), None);
    assert_eq!(Backend::parse("vulkan"), Some(Backend::Vulkan));
    assert_eq!(Backend::parse("metal"), None);
}
