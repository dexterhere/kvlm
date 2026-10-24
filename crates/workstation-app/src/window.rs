// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use gtk::{gio, glib};
use workstation_core::APP_NAME;

use crate::bridge::{self, CoreCommand, UiEvent};
use crate::views;

pub fn build(app: &adw::Application, runtime: &tokio::runtime::Handle) -> adw::ApplicationWindow {
    let (commands, events) = bridge::start(runtime);
    let runtimes = views::runtimes::Runtimes::new(commands.clone());
    let stack = adw::ViewStack::new();
    stack.set_vexpand(true);
    stack.add_titled_with_icon(
        &views::overview::build(),
        Some("overview"),
        "Overview",
        "utilities-system-monitor-symbolic",
    );
    stack.add_titled_with_icon(
        &views::catalog::build(),
        Some("catalog"),
        "Models",
        "system-search-symbolic",
    );
    stack.add_titled_with_icon(
        &runtimes.widget,
        Some("runtimes"),
        "Runtimes",
        "network-server-symbolic",
    );
    stack.add_titled_with_icon(
        &views::bench_lab::build(),
        Some("benchmarks"),
        "Benchmarks",
        "applications-science-symbolic",
    );

    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    let title = adw::WindowTitle::new(APP_NAME, "Development foundation");
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    let menu = gio::Menu::new();
    menu.append(Some("Quit"), Some("app.quit"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .build(),
    );

    let bottom_switcher = adw::ViewSwitcherBar::builder().stack(&stack).build();
    let status = gtk::Label::builder()
        .label("Checking attached Ollama…")
        .wrap(true)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .css_classes(["dim-label"])
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    toolbar.add_bottom_bar(&status);
    toolbar.add_bottom_bar(&bottom_switcher);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(APP_NAME)
        .default_width(1100)
        .default_height(720)
        .width_request(360)
        .height_request(360)
        .content(&toolbar)
        .build();
    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 700sp").expect("valid static breakpoint"),
    );
    breakpoint.add_setter(&header, "title-widget", Some(&title.to_value()));
    breakpoint.add_setter(&bottom_switcher, "reveal", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);

    let navigate = gio::SimpleAction::new("view", Some(glib::VariantTy::STRING));
    let weak_stack = stack.downgrade();
    navigate.connect_activate(move |_, parameter| {
        if let (Some(stack), Some(name)) = (weak_stack.upgrade(), parameter.and_then(|v| v.str())) {
            stack.set_visible_child_name(name);
        }
    });
    window.add_action(&navigate);
    for (index, name) in ["overview", "catalog", "runtimes", "benchmarks"]
        .into_iter()
        .enumerate()
    {
        app.set_accels_for_action(
            &format!("win.view::{name}"),
            &[&format!("<Primary>{}", index + 1)],
        );
    }

    if commands.try_send(CoreCommand::Initialize).is_err() {
        status.set_label("Background worker unavailable.");
    }
    let refresh = gio::SimpleAction::new("refresh-runtime", None);
    let weak_refresh = runtimes.refresh.downgrade();
    refresh.connect_activate(move |_, _| {
        if let Some(button) = weak_refresh.upgrade() {
            button.emit_clicked();
        }
    });
    window.add_action(&refresh);
    app.set_accels_for_action("win.refresh-runtime", &["<Primary>r"]);
    runtimes.request();
    let closing_events = events.clone();
    window.connect_close_request(move |_| {
        closing_events.close();
        glib::Propagation::Proceed
    });
    let shutdown_events = events.clone();
    app.connect_shutdown(move |_| {
        shutdown_events.close();
    });
    let weak_status = status.downgrade();
    glib::MainContext::default().spawn_local(async move {
        let _commands = commands;
        while let Ok(event) = events.recv().await {
            let Some(status) = weak_status.upgrade() else {
                break;
            };
            runtimes.receive(&event);
            match event {
                UiEvent::WorkerReady => {}
                UiEvent::OllamaDiscovered(discovery) => status.set_label(
                    if discovery.installed.is_ok() && discovery.running.is_ok() {
                        "Connected: Ollama · Attached :11434"
                    } else {
                        "Connected: Ollama · Model discovery incomplete"
                    },
                ),
                UiEvent::OllamaFailed(_) => {
                    status.set_label("Ollama unavailable · See Runtimes to retry")
                }
            }
        }
    });

    window
}
