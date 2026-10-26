// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage; normal unit tests stay display-independent.

use super::*;
use crate::bridge::{self, UiEvent};
use crate::fit_workflow::{FitReport, GpuFitReport};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use workstation_core::{
    fit::{
        gguf_fit::{LayoutAssumption, assess_gguf_fit},
        memory_math::{ExecutionTarget, GpuBackend},
    },
    models::gguf::inspect_file,
};

const TYPE_U32: u32 = 4;
const TYPE_STRING: u32 = 8;

#[test]
#[ignore = "requires a graphical GTK display; see desktop validation guide"]
fn desktop_fit_widget_flow() {
    adw::init().expect("GTK display is available");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (commands, events) = bridge::start(runtime.handle());
    let panel = ModelFit::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(600)
        .child(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&panel.widget)
                .build(),
        )
        .build();
    window.present();
    assert!(wait_for_layout(&window));
    let (minimum_width, _, _, _) = panel.widget.measure(gtk::Orientation::Horizontal, -1);
    // Tiling compositors can resize windows after GTK accepts the requested width.
    assert!(
        minimum_width <= 360,
        "content needs {minimum_width}px at minimum"
    );

    let clean = temporary_path("clean");
    let warned = temporary_path("warned");
    let malformed = temporary_path("malformed");
    write_fixture(&clean, false);
    write_fixture(&warned, true);
    fs::write(&malformed, b"not a GGUF").unwrap();

    assert!(!panel.inner.assume_layout.is_active());
    panel.inner.context_entry.set_text("257");
    panel.inner.microbatch_entry.set_text("1");
    set_selected_file(&panel.inner, clean.clone());

    panel.inner.calculate.emit_clicked();
    assert!(wait_for_fit(&panel, &events));
    assert!(
        panel
            .inner
            .metadata_label
            .label()
            .contains("Architecture declarations")
    );
    assert!(panel.inner.metadata_label.label().contains("embedding 128"));
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("Layout is unconfirmed")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("Memory breakdown unavailable")
    );

    panel.inner.assume_layout.set_active(true);
    panel.inner.calculate.emit_clicked();
    assert!(wait_for_fit(&panel, &events));
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("explicit assumption")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("KV cache: 393216")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("context 257 tokens")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("microbatch 1 tokens")
    );
    assert!(panel.inner.breakdown_label.label().contains("1 sequence"));
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("Compute buffers:")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("Runtime allowance:")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("conditional memory estimate")
    );

    assert_memory_report_rendered(&panel, &clean);
    panel.hardware_failed();
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("Hardware reading failed")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("No memory breakdown")
    );
    assert!(panel.inner.breakdown_label.tooltip_text().is_none());
    assert!(!panel.inner.status_label.has_css_class("success"));
    assert!(!panel.inner.status_label.has_css_class("dim-label"));

    set_selected_file(&panel.inner, warned.clone());
    panel.inner.calculate.emit_clicked();
    assert!(wait_for_fit(&panel, &events));
    assert!(
        panel
            .inner
            .warning_label
            .label()
            .contains("expert/MoE metadata")
    );
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("layout this estimate cannot evaluate")
    );

    set_selected_file(&panel.inner, clean.clone());
    panel.inner.calculate.emit_clicked();
    assert!(wait_for_fit(&panel, &events));
    panel.inner.context_entry.set_text("258");
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("No memory breakdown")
    );

    panel.inner.context_entry.set_text("0");
    panel.inner.calculate.emit_clicked();
    assert!(!panel.inner.state.borrow().requests.is_busy());
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("Context must be a positive")
    );

    panel.inner.context_entry.set_text("257");
    panel.inner.calculate.emit_clicked();
    panel.inner.context_entry.set_text("258");
    assert!(wait_for_fit(&panel, &events));
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("Scenario changed")
    );
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("No memory breakdown")
    );

    panel.inner.context_entry.set_text("257");
    panel.inner.calculate.emit_clicked();
    assert!(wait_for_fit(&panel, &events));
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("KV cache: 393216")
    );

    set_selected_file(&panel.inner, malformed.clone());
    panel.inner.calculate.emit_clicked();
    assert!(wait_for_fit(&panel, &events));
    assert!(panel.inner.status_label.label().contains("Estimate failed"));
    assert!(
        panel
            .inner
            .metadata_label
            .label()
            .contains("No metadata result")
    );

    window.close();
    events.close();
    drop(panel);
    runtime.shutdown_timeout(Duration::from_secs(2));
    for path in [clean, warned, malformed] {
        fs::remove_file(path).unwrap();
    }
}

#[test]
#[ignore = "requires a graphical GTK display; see desktop validation guide"]
fn desktop_fit_renders_conditional_cpu_gpu_classes() {
    adw::init().expect("GTK display is available");
    let (commands, _receiver) = tokio::sync::mpsc::channel(1);
    let panel = ModelFit::new(commands);
    let fixture = temporary_path("render");
    write_fixture(&fixture, false);

    assert_memory_report_rendered(&panel, &fixture);

    drop(panel);
    fs::remove_file(fixture).unwrap();
}

#[test]
#[ignore = "requires explicit local GGUF and native display"]
fn desktop_fit_real_qwen_acceptance() {
    adw::init().expect("GTK display is available");
    let path = std::env::var_os("KVLM_ACCEPTANCE_GGUF")
        .map(PathBuf::from)
        .expect("set KVLM_ACCEPTANCE_GGUF to an explicit local Qwen2 GGUF");
    assert!(path.is_file(), "acceptance GGUF path is not a file");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (commands, events) = bridge::start(runtime.handle());
    let panel = ModelFit::new(commands);
    let window = gtk::Window::builder()
        .default_width(1200)
        .default_height(800)
        .child(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&panel.widget)
                .build(),
        )
        .build();
    window.present();
    assert!(wait_for_layout(&window));

    set_selected_file(&panel.inner, path);
    panel.inner.assume_layout.set_active(true);
    panel.inner.key_cache.set_selected(0);
    panel.inner.value_cache.set_selected(0);
    panel.inner.microbatch_entry.set_text("512");

    for (context, expected_kv_bytes) in [(8192, 469_762_048), (32768, 1_879_048_192)] {
        panel.inner.context_entry.set_text(&context.to_string());
        if context == 32_768 {
            assert!(
                panel
                    .inner
                    .breakdown_label
                    .label()
                    .contains("No memory breakdown")
            );
        }
        panel.inner.calculate.emit_clicked();
        assert!(wait_for_real_fit(
            &panel,
            &events,
            context,
            expected_kv_bytes
        ));

        let status = panel.inner.status_label.label();
        let breakdown = panel.inner.breakdown_label.label();
        assert!(
            panel
                .inner
                .metadata_label
                .label()
                .contains("Architecture: \"qwen2\"")
        );
        assert!(
            panel
                .inner
                .metadata_label
                .label()
                .contains("Architecture declarations:")
        );
        assert!(breakdown.contains(&format!("context {context} tokens")));
        assert!(breakdown.contains("microbatch 512 tokens"));
        assert!(breakdown.contains(&format!("KV cache: {expected_kv_bytes}")));
        for component in [
            "Weights / file size:",
            "KV cache:",
            "Compute buffers:",
            "Runtime allowance:",
        ] {
            assert!(breakdown.contains(component), "missing {component}");
        }
        assert!(status.contains("Runtime compatibility remains unknown"));
        println!("Qwen2 context={context} status:\n{status}\nbreakdown:\n{breakdown}");
    }

    window.close();
    events.close();
    drop(panel);
    runtime.shutdown_timeout(Duration::from_secs(2));
}

fn assert_memory_report_rendered(panel: &ModelFit, path: &Path) {
    panel.inner.context_entry.set_text("257");
    panel.inner.microbatch_entry.set_text("1");
    panel.inner.assume_layout.set_active(true);
    let mut report = synthetic_fit_report(path);
    render_report(&panel.inner, &report);
    let rendered = panel.inner.breakdown_label.label();
    assert!(rendered.contains("CPU · Comfortable conditional memory estimate"));
    let gpu_line = rendered
        .lines()
        .find(|line| line.contains("GPU · Test CUDA (test-gpu)"))
        .unwrap();
    assert!(gpu_line.contains("currently free VRAM: 1 bytes"));
    assert!(gpu_line.contains("Not recommended conditional memory estimate"));
    assert!(rendered.contains("GPU VRAM budget limits:"));
    assert!(rendered.contains("Weights / file size:"));
    assert!(rendered.contains("KV cache:"));
    assert!(rendered.contains("Compute buffers:"));
    assert!(rendered.contains("Runtime allowance:"));
    assert!(panel.inner.breakdown_label.tooltip_text().is_some());
    let budget_tooltip = panel.inner.budget_label.tooltip_text().unwrap();
    assert!(budget_tooltip.contains("additional fully resident load"));
    assert!(budget_tooltip.contains("currently free RAM or VRAM"));
    assert!(budget_tooltip.contains("partial offload or reuse"));
    assert!(panel.inner.status_label.has_css_class("success"));
    assert!(
        panel
            .inner
            .status_label
            .label()
            .contains("Runtime compatibility remains unknown")
    );

    report.estimate.assessment.breakdown = None;
    report.estimate.assessment.limits = None;
    report.gpu_estimates[0].estimate.assessment.breakdown = None;
    report.gpu_estimates[0].estimate.assessment.limits = None;
    render_report(&panel.inner, &report);
    let rendered = panel.inner.breakdown_label.label();
    assert!(rendered.contains("CPU · Unknown conditional memory estimate"));
    let gpu_line = rendered
        .lines()
        .find(|line| line.contains("GPU · Test CUDA (test-gpu)"))
        .unwrap();
    assert!(gpu_line.contains("currently free VRAM: 1 bytes"));
    assert!(gpu_line.contains("Unknown conditional memory estimate"));
    assert!(panel.inner.status_label.has_css_class("dim-label"));

    panel.inner.context_entry.set_text("258");
    assert!(
        panel
            .inner
            .breakdown_label
            .label()
            .contains("No memory breakdown")
    );
    assert!(panel.inner.breakdown_label.tooltip_text().is_none());
    assert!(!panel.inner.status_label.has_css_class("dim-label"));
}

fn wait_for_fit(panel: &ModelFit, events: &async_channel::Receiver<UiEvent>) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(UiEvent::FitFinished { id, result }) => {
                panel.receive(id, &result);
                return true;
            }
            Ok(_) => panic!("the test does not request hardware discovery"),
            Err(async_channel::TryRecvError::Empty) => {
                pump_gtk();
                thread::sleep(Duration::from_millis(1));
            }
            Err(async_channel::TryRecvError::Closed) => return false,
        }
    }
    false
}

fn wait_for_real_fit(
    panel: &ModelFit,
    events: &async_channel::Receiver<UiEvent>,
    context: u64,
    expected_kv_bytes: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(UiEvent::FitFinished { id, result }) => {
                let report = result.as_ref().unwrap_or_else(|error| {
                    panic!("real Qwen2 estimate failed at context {context}: {error}")
                });
                assert_eq!(report.metadata.architecture.as_deref(), Some("qwen2"));
                let attention = report.metadata.attention.as_ref().unwrap();
                assert_eq!(attention.block_count, Some(28));
                assert_eq!(attention.embedding_length, Some(3584));
                assert_eq!(attention.attention_heads, Some(28));
                assert_eq!(attention.kv_heads, Some(4));
                assert!(report.available_ram_bytes.is_some());
                let cpu = report.estimate.assessment.breakdown.unwrap();
                assert_eq!(cpu.kv_cache_bytes, expected_kv_bytes);
                assert_four_parts(cpu);
                assert!(
                    !report.gpu_estimates.is_empty(),
                    "no CUDA estimate was returned"
                );
                for gpu in &report.gpu_estimates {
                    assert!(
                        gpu.available_vram_bytes.is_some(),
                        "{} has no VRAM reading",
                        gpu.name
                    );
                    let breakdown = gpu.estimate.assessment.breakdown.unwrap();
                    assert_eq!(breakdown.kv_cache_bytes, expected_kv_bytes);
                    assert_four_parts(breakdown);
                    println!(
                        "Qwen2 context={context} GPU={} id={} free_vram={:?} memory_class={:?} breakdown={breakdown:?}",
                        gpu.name,
                        gpu.id,
                        gpu.available_vram_bytes,
                        gpu.estimate.assessment.memory_class()
                    );
                }
                println!(
                    "Qwen2 context={context} CPU free_ram={:?} memory_class={:?} breakdown={cpu:?}",
                    report.available_ram_bytes,
                    report.estimate.assessment.memory_class()
                );
                panel.receive(id, &result);
                let rendered = panel.inner.breakdown_label.label();
                for gpu in &report.gpu_estimates {
                    let gpu_heading =
                        format!("GPU · {} ({}) · currently free VRAM:", gpu.name, gpu.id);
                    let gpu_start = rendered.find(&gpu_heading).unwrap();
                    let gpu_breakdown = &rendered[gpu_start..];
                    for component in [
                        "Weights / file size:",
                        "KV cache:",
                        "Compute buffers:",
                        "Runtime allowance:",
                    ] {
                        assert!(gpu_breakdown.contains(component));
                    }
                }
                assert!(rendered.contains("GPU VRAM budget limits:"));
                return true;
            }
            Ok(_) => panic!("acceptance flow received unrelated worker event"),
            Err(async_channel::TryRecvError::Empty) => {
                pump_gtk();
                thread::sleep(Duration::from_millis(1));
            }
            Err(async_channel::TryRecvError::Closed) => return false,
        }
    }
    false
}

fn assert_four_parts(breakdown: workstation_core::fit::memory_math::MemoryBreakdown) {
    assert!(breakdown.weights_bytes > 0);
    assert!(breakdown.kv_cache_bytes > 0);
    assert!(breakdown.buffer_bytes > 0);
    assert!(breakdown.overhead_bytes > 0);
    assert_eq!(
        breakdown.total_bytes,
        breakdown.weights_bytes
            + breakdown.kv_cache_bytes
            + breakdown.buffer_bytes
            + breakdown.overhead_bytes
    );
}

fn pump_gtk() {
    let context = gtk::glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
    }
}

fn wait_for_layout(window: &gtk::Window) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        pump_gtk();
        if window.width() > 0 && window.height() > 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

fn temporary_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "kvlm-model-fit-{name}-{}-{nonce}.gguf",
        std::process::id()
    ))
}

fn synthetic_fit_report(path: &Path) -> FitReport {
    let metadata = inspect_file(path).unwrap();
    let scenario = parse_scenario("257", "1", KvCacheType::F16, KvCacheType::Q8_0).unwrap();
    let layout = LayoutAssumption::UniformFullAttention;
    let estimate = assess_gguf_fit(
        &metadata,
        &scenario,
        &ExecutionTarget::Cpu {
            available_ram_bytes: Some(8 * 1024 * 1024 * 1024),
        },
        layout,
    )
    .unwrap();
    let gpu_estimate = assess_gguf_fit(
        &metadata,
        &scenario,
        &ExecutionTarget::DedicatedGpu {
            backend: GpuBackend::Cuda,
            available_vram_bytes: Some(1),
        },
        layout,
    )
    .unwrap();
    FitReport {
        metadata,
        estimate,
        available_ram_bytes: Some(8 * 1024 * 1024 * 1024),
        captured_at_unix_ms: 1,
        ram_notice: None,
        gpu_estimates: vec![GpuFitReport {
            id: "test-gpu".into(),
            name: "Test CUDA".into(),
            available_vram_bytes: Some(1),
            estimate: gpu_estimate,
        }],
        gpu_notices: vec![],
    }
}

fn write_fixture(path: &Path, warning: bool) {
    let mut metadata = vec![
        entry(b"general.architecture", TYPE_STRING, &text(b"qwen2")),
        entry(b"qwen2.block_count", TYPE_U32, &scalar(2)),
        entry(b"qwen2.embedding_length", TYPE_U32, &scalar(128)),
        entry(b"qwen2.attention.head_count", TYPE_U32, &scalar(4)),
        entry(b"qwen2.attention.head_count_kv", TYPE_U32, &scalar(2)),
        entry(b"qwen2.attention.key_length", TYPE_U32, &scalar(32)),
        entry(b"qwen2.attention.value_length", TYPE_U32, &scalar(64)),
    ];
    if warning {
        metadata.push(entry(b"qwen2.expert_count", TYPE_U32, &scalar(2)));
    }
    let mut bytes = b"GGUF".to_vec();
    u32(3, &mut bytes);
    u64(1, &mut bytes);
    u64(metadata.len() as u64, &mut bytes);
    for item in metadata {
        bytes.extend(item);
    }
    string(b"weight", &mut bytes);
    u32(1, &mut bytes);
    u64(1, &mut bytes);
    u32(0, &mut bytes);
    u64(0, &mut bytes);
    bytes.resize(bytes.len().div_ceil(32) * 32, 0);
    bytes.extend(0f32.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

fn entry(key: &[u8], kind: u32, value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    string(key, &mut output);
    u32(kind, &mut output);
    output.extend(value);
    output
}

fn text(value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    string(value, &mut output);
    output
}

fn scalar(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn string(value: &[u8], output: &mut Vec<u8>) {
    u64(value.len() as u64, output);
    output.extend(value);
}

fn u32(value: u32, output: &mut Vec<u8>) {
    output.extend(value.to_le_bytes());
}

fn u64(value: u64, output: &mut Vec<u8>) {
    output.extend(value.to_le_bytes());
}
