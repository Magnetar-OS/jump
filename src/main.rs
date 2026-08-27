// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! `jump` — a Spotlight-class launcher for COSMIC.

mod actions;
mod anim;
mod app;
mod apps;
mod clipboard;
mod launch;
mod surface;
mod system;
mod toplevel;
mod tray;
mod view;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,jump=info")),
        )
        // pop-launcher speaks JSON on stdout, and our own logs must never end
        // up interleaved with a child's protocol stream.
        .with_writer(std::io::stderr)
        .init();

    // Before the application is constructed: `fl!` is evaluated during `init`.
    jump::localize::localize();

    let settings = cosmic::app::Settings::default()
        // The overlay is a layer-shell surface created on demand, so there is
        // no xdg-toplevel to open at startup.
        .no_main_window(true)
        // Keep running after the overlay is dismissed. The whole latency
        // argument for this design rests on the next open being warm.
        .is_daemon(true)
        .exit_on_close(false)
        // The surface covers the output and is mostly empty; without this the
        // compositor would composite an opaque black rectangle over the screen.
        .transparent(true)
        .client_decorations(false)
        .resizable(None)
        .antialiasing(true);

    // `run_single_instance` claims the D-Bus name; a second invocation is
    // delivered to the running daemon as an activation, which the app turns
    // into a toggle. That makes `jump` itself the keybinding command.
    let flags = app::Flags {
        daemon: std::env::args().any(|argument| argument == "--daemon"),
    };

    cosmic::app::run_single_instance::<app::App>(settings, flags)
}
