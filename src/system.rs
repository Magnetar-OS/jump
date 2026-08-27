// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Built-in system commands.
//!
//! Covers what pop-launcher's session scripts do *not*: pop-launcher already
//! ships lock, log out, suspend, restart, shutdown and BIOS entry as scripts,
//! and those keep flowing through the service. What is missing on COSMIC is
//! everything specific to the desktop itself — flipping dark mode, and jumping
//! straight to a settings page instead of opening COSMIC Settings and clicking
//! through its sidebar.
//!
//! Commands are matched here rather than through [`jump_core::rank`]'s title
//! scoring because their vocabulary is wider than their names: "wifi" should
//! find the Wi-Fi page and "theme" should find dark mode, and neither word is
//! in the title. Each command carries its own keyword list and the match
//! quality is computed against title *and* keywords, then handed to the ranker
//! as a pre-scored item.

use jump_core::{Icon, Item, ItemKey, Source};

use crate::launch::Launch;
use jump::fl;

/// Commands only surface once this much of the query matches. Deliberately
/// strict: system commands are a convenience, and a launcher that offers
/// "Displays" for the query "d" is noise, not depth.
const MIN_SCORE: f32 = 0.45;

/// What activating a command does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Flip `is_dark` in the COSMIC theme-mode store. The whole desktop —
    /// this launcher included — follows via cosmic-settings-daemon.
    ToggleDarkMode,
    /// Open one page of COSMIC Settings, by its CLI subcommand.
    SettingsPage(&'static str),
    /// A D-Bus call that must not run on the frame.
    Background(BackgroundTask),
}

/// Work that needs a bus round trip, handed back to the caller to run as an
/// async task rather than blocking `update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTask {
    MediaPlayPause,
    MediaNext,
    MediaPrevious,
    /// Set power-profiles-daemon's active profile.
    PowerProfile(&'static str),
}

/// What the caller should do about an activated command.
#[derive(Debug)]
pub enum Outcome {
    /// Start this program; the caller owns the activation-token dance.
    Launch(Launch),
    /// Finished synchronously.
    Done,
    /// Run this off the frame, then dismiss.
    Background(BackgroundTask),
}

/// One built-in command.
struct Command {
    /// Stable identifier; becomes the [`ItemKey`] so frecency learns it.
    id: &'static str,
    title: String,
    subtitle: String,
    icon: &'static str,
    /// Match vocabulary beyond the title, lowercased.
    keywords: &'static [&'static str],
    action: Action,
}

/// The command table, rebuilt per call so titles follow a live locale change.
///
/// Small enough (a few dozen entries) that rebuilding beats caching plus the
/// invalidation story caching would need.
///
/// `now_playing` is the current track, shown as the media commands' subtitle
/// when known — [`now_playing`] fetches it off the frame when the overlay
/// opens, because a bus round trip must never run on the match path.
fn commands(now_playing: Option<&str>) -> Vec<Command> {
    let settings = |id, title: String, page, icon, keywords| Command {
        id,
        title,
        subtitle: fl!("system-settings-subtitle"),
        icon,
        keywords,
        action: Action::SettingsPage(page),
    };

    let mut commands = vec![
        Command {
            id: "dark-mode",
            title: fl!("system-dark-mode"),
            subtitle: fl!("system-dark-mode-subtitle"),
            icon: "dark-mode-symbolic",
            keywords: &["dark", "light", "mode", "theme", "appearance", "toggle"],
            action: Action::ToggleDarkMode,
        },
        settings(
            "settings-displays",
            fl!("system-page-displays"),
            "displays",
            "preferences-desktop-display-symbolic",
            &["display", "monitor", "screen", "resolution", "scale"],
        ),
        settings(
            "settings-appearance",
            fl!("system-page-appearance"),
            "appearance",
            "preferences-pop-desktop-appearance-symbolic",
            &["appearance", "theme", "accent", "color", "style"],
        ),
        settings(
            "settings-wallpaper",
            fl!("system-page-wallpaper"),
            "wallpaper",
            "preferences-desktop-wallpaper-symbolic",
            &["wallpaper", "background", "desktop"],
        ),
        settings(
            "settings-network",
            fl!("system-page-network"),
            "network",
            "preferences-system-network-symbolic",
            &["network", "wifi", "wireless", "internet", "ethernet", "vpn"],
        ),
        settings(
            "settings-bluetooth",
            fl!("system-page-bluetooth"),
            "bluetooth",
            "bluetooth-symbolic",
            &["bluetooth", "pair", "device"],
        ),
        settings(
            "settings-sound",
            fl!("system-page-sound"),
            "sound",
            "multimedia-volume-control-symbolic",
            &["sound", "audio", "volume", "output", "input", "microphone"],
        ),
        settings(
            "settings-power",
            fl!("system-page-power"),
            "power",
            "preferences-system-power-symbolic",
            &["power", "battery", "energy"],
        ),
        settings(
            "settings-keyboard",
            fl!("system-page-keyboard"),
            "keyboard",
            "input-keyboard-symbolic",
            &["keyboard", "shortcut", "keybinding", "layout", "input"],
        ),
        settings(
            "settings-mouse",
            fl!("system-page-mouse"),
            "mouse",
            "input-mouse-symbolic",
            &["mouse", "touchpad", "pointer", "cursor", "scroll"],
        ),
        settings(
            "settings-users",
            fl!("system-page-users"),
            "users",
            "system-users-symbolic",
            &["user", "account", "password"],
        ),
        settings(
            "settings-datetime",
            fl!("system-page-datetime"),
            "time",
            "preferences-system-time-symbolic",
            &["date", "time", "clock", "timezone"],
        ),
    ];

    // Media commands are always offered: players come and go mid-session, and
    // activation degrades to a log line when none is running.
    let media = |id, title: String, icon, keywords, task| Command {
        id,
        title,
        // The track that is actually playing beats a generic caption; the
        // caption stays for the moment before the async answer arrives and
        // for sessions with no player at all.
        subtitle: now_playing.map_or_else(|| fl!("system-media-subtitle"), ToOwned::to_owned),
        icon,
        keywords,
        action: Action::Background(task),
    };
    commands.push(media(
        "media-play-pause",
        fl!("system-media-play-pause"),
        "media-playback-start-symbolic",
        &["play", "pause", "resume", "music", "media"] as &[&str],
        BackgroundTask::MediaPlayPause,
    ));
    commands.push(media(
        "media-next",
        fl!("system-media-next"),
        "media-skip-forward-symbolic",
        &["next", "skip", "track", "song", "music", "media"],
        BackgroundTask::MediaNext,
    ));
    commands.push(media(
        "media-previous",
        fl!("system-media-previous"),
        "media-skip-backward-symbolic",
        &["previous", "back", "track", "song", "music", "media"],
        BackgroundTask::MediaPrevious,
    ));

    // Power profiles only when the daemon's tooling is installed; on a machine
    // without power-profiles-daemon the commands would be dead weight.
    if power_profiles_available() {
        let profile = |id, title: String, keywords, name| Command {
            id,
            title,
            subtitle: fl!("system-power-subtitle"),
            icon: "preferences-system-power-symbolic",
            keywords,
            action: Action::Background(BackgroundTask::PowerProfile(name)),
        };
        commands.push(profile(
            "power-performance",
            fl!("system-power-performance"),
            &["power", "profile", "performance", "mode"] as &[&str],
            "performance",
        ));
        commands.push(profile(
            "power-balanced",
            fl!("system-power-balanced"),
            &["power", "profile", "balanced", "mode"],
            "balanced",
        ));
        commands.push(profile(
            "power-saver",
            fl!("system-power-saver"),
            &["power", "profile", "saver", "battery", "eco", "mode"],
            "power-saver",
        ));
    }

    commands
}

/// Whether power-profiles-daemon is plausibly present.
///
/// Checked by its CLI rather than a bus round trip, because this runs on the
/// match path. Wrong at most in the "installed but disabled" direction, where
/// activation logs the failure instead.
fn power_profiles_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("powerprofilesctl").is_file())
        })
    })
}

/// How well `query` matches a command's vocabulary, in `0.0..=1.0`.
///
/// Every query token has to land somewhere — "dark window" must not match
/// dark mode — with full-word keyword hits worth more than substring title
/// hits, so "wifi" beats a stray substring.
fn match_score(command: &Command, tokens: &[String]) -> f32 {
    if tokens.is_empty() {
        return 0.0;
    }

    let title = command.title.to_lowercase();
    let mut total = 0.0;

    for token in tokens {
        // Below three characters everything prefix-matches something, and a
        // launcher offering "Displays" for "d" is noise. Real matches arrive a
        // keystroke later.
        if token.chars().count() < 3 {
            return 0.0;
        }
        let by_keyword = command
            .keywords
            .iter()
            .any(|keyword| keyword.starts_with(token.as_str()));
        let by_title = title.contains(token.as_str());

        if by_keyword {
            total += 1.0;
        } else if by_title {
            total += 0.8;
        } else {
            return 0.0;
        }
    }

    total / tokens.len() as f32
}

/// Commands matching `query`, as pre-scored [`Item`]s ready to merge.
#[must_use]
pub fn matching(query: &str, now_playing: Option<&str>) -> Vec<Item> {
    let tokens: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();

    commands(now_playing)
        .into_iter()
        .filter_map(|command| {
            let score = match_score(&command, &tokens);
            if score < MIN_SCORE {
                return None;
            }

            Some(Item {
                key: ItemKey(format!("system:{}", command.id)),
                id: 0,
                title: command.title,
                subtitle: command.subtitle,
                icon: Some(Icon::Name(command.icon.to_owned())),
                category_icon: None,
                window: None,
                source: Source::System {
                    id: command.id.to_owned(),
                },
                autocomplete: None,
                score,
            })
        })
        .collect()
}

/// What running the command at `id` amounts to.
pub fn run(id: &str) -> Option<Outcome> {
    let action = commands(None)
        .into_iter()
        .find(|command| command.id == id)
        .map(|command| command.action)?;

    Some(match action {
        Action::ToggleDarkMode => {
            toggle_dark_mode();
            Outcome::Done
        }
        Action::SettingsPage(page) => Outcome::Launch(Launch {
            app_id: "com.system76.CosmicSettings".to_owned(),
            exec: format!("cosmic-settings {page}"),
            terminal: false,
            gpu: jump_core::GpuPreference::Default,
        }),
        Action::Background(task) => Outcome::Background(task),
    })
}

/// Run a bus-bound command to completion. Failures are logged, not surfaced:
/// by the time this runs the launcher has already dismissed, and there is no
/// UI left to show an error in.
pub async fn run_background(task: BackgroundTask) {
    let result = match task {
        BackgroundTask::MediaPlayPause => media_call("PlayPause").await,
        BackgroundTask::MediaNext => media_call("Next").await,
        BackgroundTask::MediaPrevious => media_call("Previous").await,
        BackgroundTask::PowerProfile(profile) => set_power_profile(profile).await,
    };
    if let Err(error) = result {
        tracing::warn!(?task, %error, "system command failed");
    }
}

/// Call one method on the most relevant MPRIS player.
async fn media_call(method: &str) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;

    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let names: Vec<String> = bus
        .list_names()
        .await?
        .into_iter()
        .map(|name| name.to_string())
        .collect();

    let mut players = Vec::new();
    for name in names.iter().filter(|name| is_player(name)) {
        let proxy = zbus::Proxy::new(
            &connection,
            name.as_str().to_owned(),
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
        )
        .await?;
        let status: String = proxy
            .get_property("PlaybackStatus")
            .await
            .unwrap_or_default();
        players.push((proxy, status));
    }

    let Some(index) = pick_player(
        &players
            .iter()
            .map(|(_, status)| status.as_str())
            .collect::<Vec<_>>(),
    ) else {
        tracing::info!("no media player is running");
        return Ok(());
    };

    players[index].0.call_method(method, &()).await?;
    Ok(())
}

/// What the most relevant MPRIS player is playing, as "Artist — Title".
///
/// `None` when no player is running or the metadata names no track. Failures
/// degrade to `None` rather than an error: a missing subtitle is the correct
/// rendering of "nothing is playing".
pub async fn now_playing() -> Option<String> {
    match fetch_now_playing().await {
        Ok(track) => track,
        Err(error) => {
            tracing::debug!(%error, "could not read player metadata");
            None
        }
    }
}

async fn fetch_now_playing() -> zbus::Result<Option<String>> {
    let connection = zbus::Connection::session().await?;

    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let names: Vec<String> = bus
        .list_names()
        .await?
        .into_iter()
        .map(|name| name.to_string())
        .collect();

    let mut players = Vec::new();
    for name in names.iter().filter(|name| is_player(name)) {
        let proxy = zbus::Proxy::new(
            &connection,
            name.as_str().to_owned(),
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
        )
        .await?;
        let status: String = proxy
            .get_property("PlaybackStatus")
            .await
            .unwrap_or_default();
        players.push((proxy, status));
    }

    let Some(index) = pick_player(
        &players
            .iter()
            .map(|(_, status)| status.as_str())
            .collect::<Vec<_>>(),
    ) else {
        return Ok(None);
    };

    let metadata: std::collections::HashMap<String, zbus::zvariant::OwnedValue> = players[index]
        .0
        .get_property("Metadata")
        .await
        .unwrap_or_default();

    let title = metadata
        .get("xesam:title")
        .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
        .filter(|title| !title.is_empty());
    let artists: Vec<String> = metadata
        .get("xesam:artist")
        .and_then(|value| Vec::<String>::try_from(value.try_clone().ok()?).ok())
        .unwrap_or_default();

    Ok(format_track(title, &artists))
}

/// "Artist — Title", degrading to the title alone; `None` without a title.
fn format_track(title: Option<String>, artists: &[String]) -> Option<String> {
    let title = title?;
    let artist = artists.iter().find(|artist| !artist.is_empty());
    Some(match artist {
        Some(artist) => format!("{artist} — {title}"),
        None => title,
    })
}

/// Whether a bus name belongs to a real player.
///
/// `playerctld` is a controller that proxies whichever player was last active;
/// counting it would double up every real player.
fn is_player(name: &str) -> bool {
    name.starts_with("org.mpris.MediaPlayer2.") && !name.contains("playerctld")
}

/// Which player a media key should address: the one actually playing, else
/// the first — matching what hardware media keys do.
fn pick_player(statuses: &[&str]) -> Option<usize> {
    if statuses.is_empty() {
        return None;
    }
    Some(
        statuses
            .iter()
            .position(|status| *status == "Playing")
            .unwrap_or(0),
    )
}

/// Set power-profiles-daemon's active profile, the same property
/// `powerprofilesctl set` writes.
async fn set_power_profile(profile: &str) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "net.hadess.PowerProfiles",
        "/net/hadess/PowerProfiles",
        "net.hadess.PowerProfiles",
    )
    .await?;
    proxy.set_property("ActiveProfile", profile).await?;
    Ok(())
}

/// Flip the desktop between light and dark.
///
/// Writes `is_dark` in the `com.system76.CosmicTheme.Mode` store — the same
/// key COSMIC Settings' own toggle writes — so every application follows
/// through cosmic-settings-daemon, this launcher included.
fn toggle_dark_mode() {
    use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

    let store = match Config::new("com.system76.CosmicTheme.Mode", 1) {
        Ok(store) => store,
        Err(error) => {
            tracing::error!(%error, "theme-mode store unavailable; cannot toggle dark mode");
            return;
        }
    };

    // Missing key means the user has never toggled it; COSMIC defaults dark.
    let is_dark: bool = store.get("is_dark").unwrap_or(true);
    if let Err(error) = store.set("is_dark", !is_dark) {
        tracing::error!(%error, "could not write theme mode");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_for(query: &str, id: &str) -> f32 {
        let tokens: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        commands(None)
            .into_iter()
            .find(|command| command.id == id)
            .map(|command| match_score(&command, &tokens))
            .expect("command exists")
    }

    #[test]
    fn keyword_vocabulary_matches_beyond_the_title() {
        // "wifi" appears nowhere in "Network Settings".
        assert!(score_for("wifi", "settings-network") >= MIN_SCORE);
        assert!(score_for("theme", "dark-mode") >= MIN_SCORE);
    }

    #[test]
    fn every_token_must_land() {
        assert!(score_for("dark window", "dark-mode") < f32::EPSILON);
    }

    #[test]
    fn unrelated_queries_match_nothing() {
        assert!(matching("firefox", None).is_empty());
        assert!(matching("", None).is_empty());
    }

    #[test]
    fn strict_threshold_holds_back_single_letters() {
        assert!(matching("d", None).is_empty());
    }

    #[test]
    fn now_playing_becomes_the_media_subtitle() {
        let items = matching("play", Some("Boards of Canada — Roygbiv"));
        let media = items
            .iter()
            .find(|item| matches!(&item.source, Source::System { id } if id == "media-play-pause"))
            .expect("play matches the media command");
        assert_eq!(media.subtitle, "Boards of Canada — Roygbiv");
    }

    #[test]
    fn track_formatting_degrades_gracefully() {
        assert_eq!(
            format_track(Some("Roygbiv".into()), &["Boards of Canada".into()]),
            Some("Boards of Canada — Roygbiv".into())
        );
        assert_eq!(
            format_track(Some("Roygbiv".into()), &[]),
            Some("Roygbiv".into())
        );
        assert_eq!(
            format_track(Some("Roygbiv".into()), &[String::new()]),
            Some("Roygbiv".into())
        );
        assert_eq!(format_track(None, &["Someone".into()]), None);
    }

    #[test]
    fn media_keys_address_the_playing_player_first() {
        assert_eq!(pick_player(&["Paused", "Playing", "Stopped"]), Some(1));
        assert_eq!(pick_player(&["Paused", "Stopped"]), Some(0));
        assert_eq!(pick_player(&[]), None);
    }

    #[test]
    fn playerctld_is_not_a_player() {
        assert!(is_player("org.mpris.MediaPlayer2.mpv"));
        assert!(!is_player("org.mpris.MediaPlayer2.playerctld"));
        assert!(!is_player("org.freedesktop.DBus"));
    }
}
