// SPDX-License-Identifier: GPL-3.0-or-later

pub fn build() -> adw::StatusPage {
    adw::StatusPage::builder()
        .title("Benchmark Lab")
        .icon_name("applications-science-symbolic")
        .description("Quick benchmarks are planned for M0 and controlled comparisons for M3. No benchmarks have been run.")
        .build()
}
