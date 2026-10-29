// SPDX-License-Identifier: GPL-3.0-or-later
//! The app palette and its one CSS provider. Teal is primary, amber secondary. Both stylesheets
//! are embedded, so Flatpak and AppImage builds need no extra installed files.

use gtk::gdk;

const LIGHT_CSS: &str = include_str!("theme/light.css");
const DARK_CSS: &str = include_str!("theme/dark.css");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub accent_bg: &'static str,
    pub accent_fg: &'static str,
    pub accent: &'static str,
    pub teal: &'static str,
    pub kv: &'static str,
    pub amber: &'static str,
    pub brown: &'static str,
    pub neutral: &'static str,
    pub track: &'static str,
}

pub const LIGHT: Palette = Palette {
    accent_bg: "#0f766e",
    accent_fg: "#ffffff",
    accent: "#0f766e",
    teal: "#0f766e",
    kv: "#0d9488",
    amber: "#d97706",
    brown: "#8a5a00",
    neutral: "#6b7c78",
    track: "#e4ebe9",
};

pub const DARK: Palette = Palette {
    accent_bg: "#2dd4bf",
    accent_fg: "#04211e",
    accent: "#5eead4",
    teal: "#5eead4",
    kv: "#2dd4bf",
    amber: "#f5d061",
    brown: "#d99a1f",
    neutral: "#9db0ac",
    track: "#3a3a3a",
};

pub fn palette_for(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

pub fn palette() -> &'static Palette {
    palette_for(adw::StyleManager::default().is_dark())
}

pub fn color(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).expect("palette colors are valid hex values")
}

fn load(provider: &gtk::CssProvider) {
    provider.load_from_string(if adw::StyleManager::default().is_dark() {
        DARK_CSS
    } else {
        LIGHT_CSS
    });
}

/// Register the palette once, before the first window is built.
pub fn install() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INSTALLED.get().is_some() {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.connect_parsing_error(|_, _, error| {
        eprintln!("Kvlm theme CSS ignored a rule: {error}");
    });
    load(&provider);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    adw::StyleManager::default().connect_dark_notify(move |_| load(&provider));
    let _ = INSTALLED.set(());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_present(css: &str, palette: &Palette) {
        for hex in [
            palette.accent_bg,
            palette.accent_fg,
            palette.accent,
            palette.teal,
            palette.kv,
            palette.amber,
            palette.brown,
            palette.neutral,
            palette.track,
        ] {
            assert!(
                css.contains(hex),
                "{hex} is missing from the matching stylesheet"
            );
        }
    }

    #[test]
    fn stylesheets_carry_every_palette_colour() {
        assert_present(LIGHT_CSS, &LIGHT);
        assert_present(DARK_CSS, &DARK);
    }

    #[test]
    fn light_and_dark_choose_distinct_palettes() {
        assert_eq!(palette_for(false), &LIGHT);
        assert_eq!(palette_for(true), &DARK);
        assert_ne!(LIGHT.accent_bg, DARK.accent_bg);
    }

    #[test]
    fn stylesheets_are_gtk_4_12_safe() {
        for css in [LIGHT_CSS, DARK_CSS] {
            for unsupported in ["var(", "color-mix(", "oklab(", ":root"] {
                assert!(!css.contains(unsupported), "{unsupported} needs GTK 4.16+");
            }
        }
    }

    #[test]
    #[ignore = "requires a graphical GTK display"]
    fn both_stylesheets_parse() {
        adw::init().expect("GTK display is available");
        use std::{cell::RefCell, rc::Rc};
        for css in [LIGHT_CSS, DARK_CSS] {
            let provider = gtk::CssProvider::new();
            let errors = Rc::new(RefCell::new(Vec::new()));
            let sink = errors.clone();
            provider.connect_parsing_error(move |_, _, error| {
                sink.borrow_mut().push(error.to_string());
            });
            provider.load_from_string(css);
            assert!(errors.borrow().is_empty(), "{:?}", errors.borrow());
        }
    }
}
