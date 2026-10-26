// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK audit: every page builds at the minimum window width and every interactive
//! widget has a name a screen reader can announce (a label, a tooltip, a placeholder or a
//! label that points at it). Real screen-reader and GNOME-session passes are manual.

use adw::prelude::*;
use tokio::sync::mpsc;

fn labels_pointing_at(widget: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
    if let Ok(label) = widget.clone().downcast::<gtk::Label>()
        && let Some(target) = label.mnemonic_widget()
    {
        out.push(target);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        labels_pointing_at(&current, out);
        child = current.next_sibling();
    }
}

fn has_text(text: Option<impl AsRef<str>>) -> bool {
    text.is_some_and(|t| !t.as_ref().trim().is_empty())
}

fn named(widget: &gtk::Widget, pointed_at: &[gtk::Widget]) -> bool {
    if has_text(widget.tooltip_text()) || pointed_at.contains(widget) {
        return true;
    }
    if let Some(button) = widget.downcast_ref::<gtk::Button>() {
        return has_text(button.label())
            || button
                .child()
                .and_then(|c| c.downcast::<gtk::Label>().ok())
                .is_some_and(|l| has_text(Some(l.text())));
    }
    if let Some(check) = widget.downcast_ref::<gtk::CheckButton>() {
        return has_text(check.label());
    }
    if let Some(entry) = widget.downcast_ref::<gtk::SearchEntry>() {
        return has_text(entry.placeholder_text());
    }
    false
}

/// Interactive widgets with no name. Compound widgets are checked as one: their internal
/// buttons are not walked.
fn unnamed(root: &gtk::Widget) -> Vec<String> {
    let mut pointed_at = Vec::new();
    labels_pointing_at(root, &mut pointed_at);
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(widget) = stack.pop() {
        let interactive = widget.is::<gtk::Button>()
            || widget.is::<gtk::CheckButton>()
            || widget.is::<gtk::SpinButton>()
            || widget.is::<gtk::SearchEntry>()
            || widget.is::<gtk::Switch>()
            || widget.is::<gtk::DropDown>()
            || widget.is::<gtk::MenuButton>();
        // adw::ExpanderRow keeps a hidden expansion switch; hidden widgets cannot be reached.
        if interactive && widget.is_visible() && !named(&widget, &pointed_at) {
            found.push(format!("{} ({})", widget.type_().name(), widget.css_name()));
        }
        // Libadwaita rows are named by their title and own their inner switch or entry.
        if let Some(row) = widget.downcast_ref::<adw::PreferencesRow>()
            && (widget.is::<adw::SwitchRow>()
                || widget.is::<adw::ComboRow>()
                || widget.is::<adw::EntryRow>()
                || widget.is::<adw::SpinRow>())
        {
            if !has_text(Some(row.title())) {
                found.push(format!("{} without a title", widget.type_().name()));
            }
            continue;
        }
        let compound = widget.is::<gtk::SpinButton>()
            || widget.is::<gtk::SearchEntry>()
            || widget.is::<gtk::MenuButton>()
            || widget.is::<gtk::DropDown>();
        if compound {
            continue;
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            stack.push(current.clone());
            child = current.next_sibling();
        }
    }
    found
}

fn check(name: &str, widget: &gtk::Widget) {
    let missing = unnamed(widget);
    assert!(missing.is_empty(), "{name}: unnamed controls {missing:?}");
    let (min_width, ..) = widget.measure(gtk::Orientation::Horizontal, -1);
    assert!(
        min_width <= 360,
        "{name}: needs {min_width} pixels; widest parts: {:?}",
        wide_parts(widget)
    );
}

/// The deepest widgets that alone need more than the minimum width, to point at the cause.
fn wide_parts(root: &gtk::Widget) -> Vec<String> {
    let mut deepest = Vec::new();
    let mut child = root.first_child();
    while let Some(current) = child {
        if current.measure(gtk::Orientation::Horizontal, -1).0 > 360 {
            let inner = wide_parts(&current);
            if inner.is_empty() {
                let mut kids = Vec::new();
                let mut k = current.first_child();
                while let Some(c) = k {
                    kids.push(format!(
                        "{}={}",
                        c.type_().name(),
                        c.measure(gtk::Orientation::Horizontal, -1).0
                    ));
                    k = c.next_sibling();
                }
                deepest.push(format!("{} {kids:?}", current.type_().name()));
            }
            deepest.extend(inner);
        }
        child = current.next_sibling();
    }
    deepest
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn every_page_has_named_controls_and_fits_the_minimum_width() {
    adw::init().expect("GTK display is available");
    let (commands, _rx) = mpsc::channel(8);
    let (lab, _lab_rx) = mpsc::channel(8);
    let (other, _other_rx) = mpsc::channel(8);

    let overview = super::overview::Overview::new(commands.clone());
    let tokio = tokio::runtime::Runtime::new().expect("test runtime");
    overview.append(&super::updates::new(tokio.handle()));
    check("Overview", overview.widget.upcast_ref());
    let runtimes = super::runtimes::Runtimes::new(commands.clone());
    let attached = super::attached::Attached::new(other);
    runtimes.append(&attached.widget);
    check("Runtimes", runtimes.widget.upcast_ref());
    let models = super::catalog::Explorer::new(commands.clone());
    check("Models", models.widget.upcast_ref());
    let benchmarks = super::bench_lab::Benchmarks::new(commands);
    check("Benchmarks", benchmarks.widget.upcast_ref());
    let lab = super::lab::Lab::new(lab);
    check("Lab", &lab.widget);
}
