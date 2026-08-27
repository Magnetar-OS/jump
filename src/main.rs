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
    // into a toggle. That makes `jump` itself the keybinding command, and
    // `jump show <query>` a deep link into a pre-filled search — bind it to a
    // COSMIC custom shortcut for a per-command hotkey.
    let mut flags = app::Flags::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--daemon" => flags.daemon = true,
            "show" => {
                flags.action = Some(app::ACTION_SHOW.to_owned());
                flags.args = arguments.collect();
                break;
            }
            other => tracing::warn!(argument = other, "ignoring unknown argument"),
        }
    }

    cosmic::app::run_single_instance::<app::App>(settings, flags)
}
