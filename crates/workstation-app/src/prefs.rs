// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-user preferences: one small text file in the user's config directory. Nothing here is
//! sent anywhere. A missing or unreadable file means the defaults.

use std::{fs, path::PathBuf};

use gtk::glib;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow the system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|theme| theme.key() == value.trim())
            .unwrap_or_default()
    }

    pub fn apply(self) {
        adw::StyleManager::default().set_color_scheme(match self {
            Self::System => adw::ColorScheme::Default,
            Self::Light => adw::ColorScheme::ForceLight,
            Self::Dark => adw::ColorScheme::ForceDark,
        });
    }
}

fn file() -> PathBuf {
    glib::user_config_dir().join("local-ai-workstation/preferences")
}

fn read(text: &str) -> Theme {
    text.lines()
        .find_map(|line| line.strip_prefix("theme="))
        .map(Theme::parse)
        .unwrap_or_default()
}

pub fn theme() -> Theme {
    fs::read_to_string(file())
        .map(|text| read(&text))
        .unwrap_or_default()
}

/// Remember and apply the choice. A failed write only means it is not remembered.
pub fn set_theme(theme: Theme) {
    theme.apply();
    let path = file();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, format!("theme={}\n", theme.key()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_known_values_and_falls_back_to_system() {
        assert_eq!(read("theme=dark\n"), Theme::Dark);
        assert_eq!(read("other=1\ntheme=light"), Theme::Light);
        assert_eq!(read("theme=purple"), Theme::System);
        assert_eq!(read(""), Theme::System);
    }
}
