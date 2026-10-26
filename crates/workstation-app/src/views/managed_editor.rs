// SPDX-License-Identifier: GPL-3.0-or-later
//! The launch profile editor. Every field is typed; there is no free-text argument field, and
//! `LaunchProfile::validate` has the last word before anything is saved.

use adw::prelude::*;
use workstation_core::runtime::profile::{CacheType, ExtraFlag, LaunchProfile};

const CACHE_TYPES: [CacheType; 3] = [CacheType::F16, CacheType::Q8_0, CacheType::Q4_0];

/// The editor's values as plain data, so conversion is testable without a display.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Form {
    pub name: String,
    pub model_path: String,
    pub port: u32,
    pub context_length: u32,
    pub gpu_layers: u32,
    pub cache_k: usize,
    pub cache_v: usize,
    pub flash_attention: bool,
    pub ubatch_size: u32,
    /// Zero means "let llama.cpp decide" and adds no flag.
    pub threads: u32,
    pub parallel: u32,
    pub no_mmap: bool,
    pub mlock: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            name: String::new(),
            model_path: String::new(),
            port: 8081,
            context_length: 8192,
            gpu_layers: 0,
            cache_k: 0,
            cache_v: 0,
            flash_attention: false,
            ubatch_size: 512,
            threads: 0,
            parallel: 0,
            no_mmap: false,
            mlock: false,
        }
    }
}

pub(super) fn profile_from_form(form: &Form) -> Result<LaunchProfile, String> {
    let cache = |index: usize| {
        CACHE_TYPES
            .get(index)
            .copied()
            .ok_or_else(|| "Choose a cache type.".to_string())
    };
    let port = u16::try_from(form.port).map_err(|_| "Port must be 1024 to 65535.".to_string())?;
    let mut extra = Vec::new();
    if form.threads > 0 {
        extra.push(ExtraFlag::Threads(
            u16::try_from(form.threads).map_err(|_| "Too many threads.".to_string())?,
        ));
    }
    if form.parallel > 0 {
        extra.push(ExtraFlag::Parallel(
            u8::try_from(form.parallel).map_err(|_| "Too many parallel slots.".to_string())?,
        ));
    }
    if form.no_mmap {
        extra.push(ExtraFlag::NoMmap);
    }
    if form.mlock {
        extra.push(ExtraFlag::Mlock);
    }
    let profile = LaunchProfile {
        name: form.name.trim().to_owned(),
        model_path: form.model_path.trim().to_owned(),
        port,
        context_length: form.context_length,
        gpu_layers: u16::try_from(form.gpu_layers)
            .map_err(|_| "GPU layers must be 999 or fewer.".to_string())?,
        cache_type_k: cache(form.cache_k)?,
        cache_type_v: cache(form.cache_v)?,
        flash_attention: form.flash_attention,
        ubatch_size: form.ubatch_size,
        extra,
    };
    profile.validate()?;
    Ok(profile)
}

fn form_from(profile: &LaunchProfile) -> Form {
    let index = |cache: CacheType| CACHE_TYPES.iter().position(|c| *c == cache).unwrap_or(0);
    let mut form = Form {
        name: profile.name.clone(),
        model_path: profile.model_path.clone(),
        port: profile.port.into(),
        context_length: profile.context_length,
        gpu_layers: profile.gpu_layers.into(),
        cache_k: index(profile.cache_type_k),
        cache_v: index(profile.cache_type_v),
        flash_attention: profile.flash_attention,
        ubatch_size: profile.ubatch_size,
        ..Form::default()
    };
    for flag in &profile.extra {
        match flag {
            ExtraFlag::Threads(n) => form.threads = (*n).into(),
            ExtraFlag::Parallel(n) => form.parallel = (*n).into(),
            ExtraFlag::NoMmap => form.no_mmap = true,
            ExtraFlag::Mlock => form.mlock = true,
        }
    }
    form
}

fn spin(title: &str, subtitle: &str, min: f64, max: f64, value: u32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_value(f64::from(value));
    row
}

fn cache_row(title: &str, selected: usize) -> adw::ComboRow {
    let row = adw::ComboRow::builder().title(title).build();
    row.set_model(Some(&gtk::StringList::new(&["f16", "q8_0", "q4_0"])));
    row.set_selected(selected as u32);
    row
}

fn switch(title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .active(active)
        .build()
}

pub(super) fn open(
    parent: Option<&gtk::Window>,
    existing: Option<LaunchProfile>,
    on_save: impl Fn(LaunchProfile) + 'static,
) {
    let editing = existing.is_some();
    let start = existing.as_ref().map(form_from).unwrap_or_default();
    let name = adw::EntryRow::builder().title("Profile name").build();
    name.set_text(&start.name);
    // A name identifies the saved profile; renaming means making a new one.
    name.set_editable(!editing);
    let path = adw::EntryRow::builder().title("Model file (GGUF)").build();
    path.set_text(&start.model_path);
    let browse = gtk::Button::with_mnemonic("_Browse");
    browse.set_valign(gtk::Align::Center);
    path.add_suffix(&browse);
    let port = spin(
        "Port",
        "1024 to 65535, reachable only from this computer",
        1024.0,
        65535.0,
        start.port,
    );
    let context = spin(
        "Context length",
        "Tokens (256 to 1,048,576)",
        256.0,
        1_048_576.0,
        start.context_length,
    );
    let layers = spin(
        "GPU layers",
        "0 keeps everything on the CPU",
        0.0,
        999.0,
        start.gpu_layers,
    );
    let cache_k = cache_row("K cache type", start.cache_k);
    let cache_v = cache_row(
        "V cache type (quantized needs flash attention)",
        start.cache_v,
    );
    let flash = switch(
        "Flash attention",
        "Required for a quantized V cache",
        start.flash_attention,
    );
    let ubatch = spin(
        "Micro-batch size",
        "1 to 8192",
        1.0,
        8192.0,
        start.ubatch_size,
    );
    let threads = spin(
        "CPU threads",
        "0 lets llama.cpp choose",
        0.0,
        1024.0,
        start.threads,
    );
    let parallel = spin(
        "Parallel slots",
        "0 uses the llama.cpp default",
        0.0,
        64.0,
        start.parallel,
    );
    let no_mmap = switch(
        "Disable memory mapping",
        "Loads the whole file into memory",
        start.no_mmap,
    );
    let mlock = switch(
        "Lock model in memory",
        "May need a raised memory-lock limit",
        start.mlock,
    );

    let group = adw::PreferencesGroup::new();
    for row in [
        name.upcast_ref::<gtk::Widget>(),
        path.upcast_ref(),
        port.upcast_ref(),
        context.upcast_ref(),
        layers.upcast_ref(),
        cache_k.upcast_ref(),
        cache_v.upcast_ref(),
        flash.upcast_ref(),
        ubatch.upcast_ref(),
        threads.upcast_ref(),
        parallel.upcast_ref(),
        no_mmap.upcast_ref(),
        mlock.upcast_ref(),
    ] {
        group.add(row);
    }
    let error = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .css_classes(["error"])
        .visible(false)
        .selectable(true)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&error);
    body.append(&group);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&adw::Clamp::builder().maximum_size(640).child(&body).build())
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let save = gtk::Button::with_mnemonic("_Save");
    save.add_css_class("suggested-action");
    let cancel = gtk::Button::with_mnemonic("_Cancel");
    let header = adw::HeaderBar::new();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroll));
    let window = gtk::Window::builder()
        .title(if editing {
            "Edit launch profile"
        } else {
            "New launch profile"
        })
        .modal(true)
        .default_width(560)
        .default_height(640)
        .child(&toolbar)
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }
    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let weak = window.downgrade();
    browse.connect_clicked(move |_| {
        let (Some(window), true) = (weak.upgrade(), true) else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title("Choose a GGUF model file")
            .modal(true)
            .accept_label("Choose")
            .build();
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("GGUF files (*.gguf)"));
        filter.add_pattern("*.gguf");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&filter));
        let entry = path_entry(&window);
        dialog.open(
            Some(&window),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                if let (Ok(file), Some(entry)) = (result, entry)
                    && let Some(chosen) = file.path()
                {
                    entry.set_text(&chosen.to_string_lossy());
                }
            },
        );
    });
    let weak = window.downgrade();
    save.connect_clicked(move |_| {
        let form = Form {
            name: name.text().into(),
            model_path: path.text().into(),
            port: port.value() as u32,
            context_length: context.value() as u32,
            gpu_layers: layers.value() as u32,
            cache_k: cache_k.selected() as usize,
            cache_v: cache_v.selected() as usize,
            flash_attention: flash.is_active(),
            ubatch_size: ubatch.value() as u32,
            threads: threads.value() as u32,
            parallel: parallel.value() as u32,
            no_mmap: no_mmap.is_active(),
            mlock: mlock.is_active(),
        };
        match profile_from_form(&form) {
            Ok(profile) => {
                on_save(profile);
                if let Some(window) = weak.upgrade() {
                    window.close();
                }
            }
            Err(message) => {
                error.set_text(&message);
                error.set_visible(true);
            }
        }
    });
    window.present();
}

/// The model-path entry, found again by name inside the editor window.
fn path_entry(window: &gtk::Window) -> Option<adw::EntryRow> {
    fn find(widget: &gtk::Widget) -> Option<adw::EntryRow> {
        if let Some(row) = widget.downcast_ref::<adw::EntryRow>()
            && row.title() == "Model file (GGUF)"
        {
            return Some(row.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = find(&current) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }
    window.child().and_then(|child| find(&child))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Form {
        Form {
            name: "tiny".into(),
            model_path: "/models/tiny.gguf".into(),
            ..Form::default()
        }
    }

    #[test]
    fn a_valid_form_becomes_a_profile_with_only_typed_flags() {
        let mut form = valid();
        form.threads = 4;
        form.no_mmap = true;
        let profile = profile_from_form(&form).unwrap();
        assert_eq!(profile.name, "tiny");
        assert_eq!(profile.port, 8081);
        assert_eq!(
            profile.extra,
            vec![ExtraFlag::Threads(4), ExtraFlag::NoMmap]
        );
        assert_eq!(form_from(&profile), form);
    }

    #[test]
    fn the_validator_has_the_last_word() {
        for mutate in [
            (|f: &mut Form| f.name = "bad name".into()) as fn(&mut Form),
            |f| f.model_path = "relative.gguf".into(),
            |f| f.port = 80,
            |f| f.context_length = 10,
            |f| f.cache_v = 1,
            |f| f.cache_k = 9,
            |f| f.threads = 100_000,
        ] {
            let mut form = valid();
            mutate(&mut form);
            assert!(profile_from_form(&form).is_err(), "{form:?}");
        }
        let mut quantized = valid();
        quantized.cache_v = 1;
        quantized.flash_attention = true;
        assert!(profile_from_form(&quantized).is_ok());
    }
}
