// SPDX-License-Identifier: GPL-3.0-or-later

mod application;
mod attached;
mod bridge;
mod fit_state;
mod fit_workflow;
mod lab;
mod managed;
mod prefs;
mod views;
mod window;

fn main() -> gtk::glib::ExitCode {
    // Headless: print what the probes found, without device identifiers, then exit.
    if std::env::args().nth(1).as_deref() == Some("--report-hardware") {
        let profile = workstation_core::hardware::HardwareProfile::detect();
        print!(
            "{}",
            workstation_core::hardware::report::redacted_report(
                &profile,
                env!("CARGO_PKG_VERSION")
            )
        );
        return gtk::glib::ExitCode::SUCCESS;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Could not start background workers: {error}");
            return gtk::glib::ExitCode::FAILURE;
        }
    };

    let result = application::run(runtime.handle());
    managed::shutdown_blocking(runtime.handle());
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
