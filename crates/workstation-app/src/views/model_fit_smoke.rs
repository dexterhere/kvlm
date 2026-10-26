// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage; normal unit tests stay display-independent.

use super::*;
use crate::bridge::{self, UiEvent};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
    assert!(
        window.width() <= 360,
        "narrow layout grew to {} pixels",
        window.width()
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

fn write_fixture(path: &Path, warning: bool) {
    let mut metadata = vec![
        entry(b"general.architecture", TYPE_STRING, &text(b"llama")),
        entry(b"llama.block_count", TYPE_U32, &scalar(2)),
        entry(b"llama.embedding_length", TYPE_U32, &scalar(128)),
        entry(b"llama.attention.head_count", TYPE_U32, &scalar(4)),
        entry(b"llama.attention.head_count_kv", TYPE_U32, &scalar(2)),
        entry(b"llama.attention.key_length", TYPE_U32, &scalar(32)),
        entry(b"llama.attention.value_length", TYPE_U32, &scalar(64)),
    ];
    if warning {
        metadata.push(entry(b"llama.expert_count", TYPE_U32, &scalar(2)));
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
