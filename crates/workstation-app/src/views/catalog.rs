// SPDX-License-Identifier: GPL-3.0-or-later

pub fn build() -> adw::StatusPage {
    adw::StatusPage::builder()
        .title("Model Explorer & Catalog")
        .icon_name("system-search-symbolic")
        .description("Browse model families, releases, and artifacts in M1. Catalog search and downloads are not available yet.")
        .build()
}
