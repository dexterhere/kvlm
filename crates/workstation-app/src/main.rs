// SPDX-License-Identifier: GPL-3.0-or-later

mod application;
mod attached;
mod bridge;
mod fit_state;
mod fit_workflow;
mod lab;
mod managed;
mod views;
mod window;

fn main() -> gtk::glib::ExitCode {
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
