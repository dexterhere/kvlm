// SPDX-License-Identifier: GPL-3.0-or-later

pub fn build() -> adw::StatusPage {
    adw::StatusPage::builder()
        .title("Overview & Monitor")
        .icon_name("utilities-system-monitor-symbolic")
        .description("Hardware discovery and live memory telemetry are planned for M0. No hardware has been probed yet.")
        .build()
}
