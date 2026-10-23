// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use gtk::{gio, glib};
use workstation_core::APP_NAME;

use crate::bridge::{self, CoreCommand, UiEvent};
use crate::views;

pub fn build(app: &adw::Application, runtime: &tokio::runtime::Handle) -> adw::ApplicationWindow {
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
        &views::runtimes::build(),
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
        .label("Starting background worker…")
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

    let (commands, events) = bridge::start(runtime);
    if commands.send(CoreCommand::Initialize).is_err() {
        status.set_label("Background worker unavailable.");
    }
    let closing_events = events.clone();
    window.connect_close_request(move |_| {
        closing_events.close();
        glib::Propagation::Proceed
    });
    let weak_status = status.downgrade();
    glib::MainContext::default().spawn_local(async move {
        let _commands = commands;
        while let Ok(event) = events.recv().await {
            let Some(status) = weak_status.upgrade() else {
                break;
            };
            match event {
                UiEvent::WorkerReady => {
                    status.set_label("Development foundation · No runtime connected")
                }
            }
        }
    });

    window
}
