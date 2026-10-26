// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
use workstation_core::{
    export::{continue_dev, opencode, systemd, zed},
    runtime::profile::{CacheType, ExtraFlag, LaunchProfile},
};

const HOSTILE: &str = "/models/we ird \"q\" 'a' %h $HOME \\n/m.gguf";

fn profile(path: &str) -> LaunchProfile {
    LaunchProfile {
        name: "my-profile.1".into(),
        model_path: path.into(),
        port: 8081,
        context_length: 8192,
        gpu_layers: 20,
        cache_type_k: CacheType::F16,
        cache_type_v: CacheType::F16,
        flash_attention: false,
        ubatch_size: 512,
        extra: vec![ExtraFlag::Threads(4)],
    }
}

#[test]
fn json_exports_parse_and_carry_the_endpoint_and_model_name() {
    let zed: Value =
        serde_json::from_str(&zed::settings_fragment(&profile(HOSTILE)).unwrap()).unwrap();
    let provider = &zed["language_models"]["openai_compatible"]["llama.cpp (local)"];
    assert_eq!(provider["api_url"], "http://127.0.0.1:8081/v1");
    assert_eq!(provider["available_models"][0]["name"], "my-profile.1");
    let open: Value =
        serde_json::from_str(&opencode::provider_fragment(&profile(HOSTILE)).unwrap()).unwrap();
    assert_eq!(
        open["provider"]["llamacpp"]["options"]["baseURL"],
        "http://127.0.0.1:8081/v1"
    );
    assert!(open["provider"]["llamacpp"]["models"]["my-profile.1"].is_object());
}

#[test]
fn continue_entry_uses_quoted_scalars_and_the_loopback_endpoint() {
    let text = continue_dev::model_entry(&profile(HOSTILE)).unwrap();
    assert!(text.contains("name: \"my-profile.1\""));
    assert!(text.contains("apiBase: \"http://127.0.0.1:8081/v1\""));
}

#[test]
fn invalid_profiles_are_refused_by_every_export() {
    let mut bad = profile("relative/model.gguf");
    assert!(continue_dev::model_entry(&bad).is_err());
    assert!(zed::settings_fragment(&bad).is_err());
    assert!(opencode::provider_fragment(&bad).is_err());
    bad.model_path = "/m/a\nb.gguf".into();
    assert!(systemd::user_unit(&bad, "/opt/llama-server").is_err());
}

#[test]
fn unit_text_escapes_specifiers_variables_and_quotes() {
    let unit = systemd::user_unit(&profile(HOSTILE), "/opt/ll ama/llama-server").unwrap();
    assert!(unit.contains("%%h $$HOME"));
    assert!(unit.contains("$$HOME"));
    assert!(unit.contains("\\\"q\\\""));
    assert!(unit.contains("ExecStart=\"/opt/ll ama/llama-server\" "));
    assert!(unit.contains("\"127.0.0.1\""));
    assert!(systemd::user_unit(&profile(HOSTILE), "llama-server").is_err());
    assert!(systemd::user_unit(&profile(HOSTILE), "/opt/../llama-server").is_err());
    assert!(systemd::user_unit(&profile(HOSTILE), "/opt/a%h/llama-server").is_err());
    assert!(systemd::user_unit(&profile(HOSTILE), "/opt/a:b/llama-server").is_err());
    assert!(systemd::user_unit(&profile(HOSTILE), "/opt/a;b/llama-server").is_err());
}

#[test]
fn systemd_analyze_accepts_a_unit_with_hostile_paths() {
    let Ok(probe) = Command::new("systemd-analyze").arg("--version").output() else {
        return; // not installed: the text assertions above still run
    };
    if !probe.status.success() {
        return;
    }
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("unit-{}", std::process::id()));
    let bin_dir = dir.join("we ird dir");
    fs::create_dir_all(&bin_dir).unwrap();
    let bin = bin_dir.join("llama-server");
    fs::write(&bin, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let unit = systemd::user_unit(&profile(HOSTILE), bin.to_str().unwrap()).unwrap();
    let file = dir.join("llama-test.service");
    fs::write(&file, unit).unwrap();
    let out = Command::new("systemd-analyze")
        .args(["--user", "verify"])
        .arg(&file)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
