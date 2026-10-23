// SPDX-License-Identifier: GPL-3.0-or-later

pub fn build() -> adw::StatusPage {
    adw::StatusPage::builder()
        .title("Runtimes & Endpoints")
        .icon_name("network-server-symbolic")
        .description("Attached Ollama discovery is planned for M0. Runtime connections and process management are not available yet.")
        .build()
}
