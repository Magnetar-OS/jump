// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! `jump` — a Spotlight-class launcher for COSMIC.

mod actions;
mod anim;
mod app;
mod apps;
mod clipboard;
mod devices;
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

    // `jump plugin …` is plugin tooling, not a launcher invocation: it runs
    // and exits without touching the daemon. CLI output stays untranslated,
    // like logs — it is grepped, piped and pasted into bug reports.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "plugin")
    {
        plugin_cli(&arguments[1..]);
    }

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

/// `jump plugin new <name>` and `jump plugin lint <dir-or-name> [query]`.
fn plugin_cli(arguments: &[String]) -> ! {
    let user_plugins = || {
        dirs::data_dir()
            .map(|dir| dir.join("jump").join("plugins"))
            .expect("no XDG data directory")
    };

    let code = match arguments.first().map(String::as_str) {
        Some("new") => match arguments.get(1) {
            Some(name) => {
                let keyword = name.to_lowercase().replace(char::is_whitespace, "-");
                let directory = user_plugins().join(&keyword);
                match jump_core::plugin::scaffold(&directory, name) {
                    Ok(()) => {
                        println!("Created {}", directory.display());
                        println!("Try it: type “{keyword} hello” in the launcher.");
                        println!("Check it: jump plugin lint {}", directory.display());
                        0
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        1
                    }
                }
            }
            None => {
                eprintln!("usage: jump plugin new <name>");
                2
            }
        },

        Some("lint") => match arguments.get(1) {
            Some(target) => {
                // A path is used as given; a bare name is looked up in the
                // user's plugin directory.
                let mut directory = std::path::PathBuf::from(target);
                if !directory.is_dir() {
                    directory = user_plugins().join(target);
                }
                let sample = arguments.get(2).map_or("test", String::as_str);

                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                let report = runtime.block_on(jump_core::plugin::lint(&directory, sample));

                for error in &report.errors {
                    println!("error: {error}");
                }
                for warning in &report.warnings {
                    println!("warning: {warning}");
                }
                if report.is_clean() {
                    println!(
                        "ok: {} — sample query {sample:?} produced {} item(s)",
                        directory.display(),
                        report.items
                    );
                    0
                } else {
                    1
                }
            }
            None => {
                eprintln!("usage: jump plugin lint <directory-or-name> [sample-query]");
                2
            }
        },

        _ => {
            eprintln!("usage: jump plugin new <name> | jump plugin lint <dir-or-name> [query]");
            2
        }
    };
    std::process::exit(code)
}
