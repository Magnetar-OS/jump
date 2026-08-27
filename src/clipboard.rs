// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Clipboard history, over `wlr-data-control`.
//!
//! ## Why this protocol
//!
//! The ordinary `wl_data_device` only delivers a selection to the surface that
//! currently has keyboard focus, which makes it useless for a history daemon:
//! the launcher is not focused while the user copies things in another
//! application. `zwlr_data_control_manager_v1` exists precisely for clipboard
//! managers — it delivers every selection change regardless of focus. COSMIC
//! advertises it, so no fallback path is needed here.
//!
//! ## Why a thread, and why the reads are off it
//!
//! Same reason as [`crate::toplevel`]: iced owns the application's Wayland
//! connection. Reading an offer is the subtle part — the protocol hands back a
//! pipe, and the *other* application writes into it. Reading that pipe on the
//! Wayland thread would deadlock the moment a source is slow or dies, taking
//! clipboard history down with it, so each read runs on its own short-lived
//! thread under a timeout.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use cosmic::cctk::wayland_client::globals::{GlobalListContents, registry_queue_init};
use cosmic::cctk::wayland_client::protocol::{wl_registry, wl_seat};
use cosmic::cctk::wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use tokio::sync::mpsc;
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

/// MIME types we know how to store, best first.
const TEXT_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain;charset=UTF-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];

/// Entries kept. Old enough entries stop being useful long before this, but the
/// cost of holding them is a few kilobytes.
const HISTORY_LIMIT: usize = 200;

/// Largest clipboard entry stored, in bytes.
///
/// Copying a large file into the clipboard is common and there is no value in
/// keeping megabytes of it in a launcher's history.
const MAX_ENTRY: usize = 64 * 1024;

/// How long a source gets to write its data before the read is abandoned.
const READ_TIMEOUT: Duration = Duration::from_millis(500);

/// One remembered clipboard entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub text: String,
}

impl Entry {
    /// Single-line preview for the result row.
    #[must_use]
    pub fn preview(&self) -> String {
        let collapsed = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.chars().count() > 120 {
            let truncated: String = collapsed.chars().take(120).collect();
            format!("{truncated}…")
        } else {
            collapsed
        }
    }

    /// Short description of the entry's shape, shown as the subtitle.
    #[must_use]
    pub fn describe(&self) -> String {
        let lines = self.text.lines().count();
        let chars = self.text.chars().count();
        if lines > 1 {
            format!("Clipboard — {lines} lines, {chars} characters")
        } else {
            format!("Clipboard — {chars} characters")
        }
    }
}

/// Handle for putting entries back on the clipboard.
///
/// Copying runs on its own thread with its own Wayland connection, for two
/// reasons learned the hard way. The watcher thread parks inside
/// `blocking_dispatch`, so a request sent to it sits unread until some
/// unrelated clipboard event happens to arrive — a copy that takes effect
/// "eventually" is a copy that did not happen. And serving a selection from
/// the watcher makes the watcher read its *own* offer on the thread that has
/// to write it, which can only ever end in a timeout. A separate connection
/// makes the server just another client: the watcher records the copy into
/// history through the normal path, and the server exists exactly as long as
/// the ownership does — the compositor's `Cancelled` event is the exit signal.
#[derive(Debug, Clone)]
pub struct Clipboard;

impl Clipboard {
    /// Set the system clipboard to `text`.
    ///
    /// Returns once the compositor has acknowledged the selection, so a copy
    /// followed immediately by dismissal cannot lose the race.
    pub fn copy(&self, text: &str) {
        serve_copy(text.to_owned());
    }
}

/// Take the clipboard and keep serving `text` until another application
/// replaces the selection.
fn serve_copy(text: String) {
    let Ok(connection) = Connection::connect_to_env() else {
        tracing::warn!("clipboard: cannot connect to the display to copy");
        return;
    };

    let Ok((globals, mut queue)) = registry_queue_init::<CopyServer>(&connection) else {
        tracing::warn!("clipboard: failed to initialise the Wayland registry");
        return;
    };
    let qh = queue.handle();

    let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 1..=2, ()) else {
        tracing::warn!("clipboard: wlr-data-control unavailable; cannot copy");
        return;
    };
    let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=8, ()) else {
        tracing::warn!("clipboard: no seat available; cannot copy");
        return;
    };

    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    for mime in TEXT_MIMES {
        source.offer((*mime).to_owned());
    }
    device.set_selection(Some(&source));

    let mut server = CopyServer {
        text,
        cancelled: false,
    };

    // Block until the compositor has seen the selection before returning to
    // the caller; the serving itself then moves to a background thread.
    if queue.roundtrip(&mut server).is_err() {
        tracing::warn!("clipboard: connection lost while copying");
        return;
    }
    tracing::debug!(bytes = server.text.len(), "clipboard: took the selection");

    std::thread::Builder::new()
        .name("jump-clipboard-copy".into())
        .spawn(move || {
            while !server.cancelled {
                if queue.blocking_dispatch(&mut server).is_err() {
                    tracing::warn!("clipboard: connection closed while serving");
                    return;
                }
            }
            tracing::debug!("clipboard: selection replaced; serving thread exits");
        })
        .ok();
}

/// State of one copy-serving connection.
struct CopyServer {
    text: String,
    cancelled: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for CopyServer {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for CopyServer {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for CopyServer {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for CopyServer {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The device announces offers, ours included; a copy server has no
        // interest in reading anything. Offers are destroyed once `selection`
        // has announced them — the safe point, after their MIME events — so
        // the connection does not accumulate dead objects while serving.
        match event {
            zwlr_data_control_device_v1::Event::Selection { id: Some(offer) }
            | zwlr_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                offer.destroy();
            }
            _ => {}
        }
    }

    // `data_offer` creates a child object; without this specialization
    // wayland-client panics inside dispatch, which cannot unwind, and the
    // whole daemon aborts. Measured, not theoretical.
    cosmic::cctk::wayland_client::event_created_child!(CopyServer, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for CopyServer {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlOfferV1,
        _: <ZwlrDataControlOfferV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for CopyServer {
    fn event(
        server: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Send { fd, .. } => {
                tracing::debug!("clipboard: send requested");
                let mut file = std::fs::File::from(fd);
                if let Err(error) = std::io::Write::write_all(&mut file, server.text.as_bytes()) {
                    tracing::debug!(%error, "clipboard: paste target closed early");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                server.cancelled = true;
                source.destroy();
            }
            _ => {}
        }
    }
}

/// Start watching the clipboard.
///
/// Returns a handle plus a stream of history snapshots. `None` when the
/// compositor does not implement `wlr-data-control`.
pub fn spawn() -> Option<(Clipboard, mpsc::UnboundedReceiver<Vec<Entry>>)> {
    let connection = Connection::connect_to_env().ok()?;
    let (history_tx, history_rx) = mpsc::unbounded_channel();

    std::thread::Builder::new()
        .name("jump-clipboard".into())
        .spawn(move || run(&connection, &history_tx))
        .ok()?;

    Some((Clipboard, history_rx))
}

fn run(connection: &Connection, history: &mpsc::UnboundedSender<Vec<Entry>>) {
    let Ok((globals, mut queue)) = registry_queue_init::<State>(connection) else {
        tracing::warn!("clipboard: failed to initialise the Wayland registry");
        return;
    };
    let qh = queue.handle();

    let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 1..=2, ()) else {
        tracing::info!("compositor does not implement wlr-data-control; clipboard history off");
        return;
    };
    let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=8, ()) else {
        tracing::warn!("clipboard: no seat available");
        return;
    };

    let device = manager.get_data_device(&seat, &qh, ());

    // The manager and device stay bound for the lifetime of the loop even
    // though nothing references them after setup: dropping the Rust handles
    // would not destroy the server objects, but keeping them is clearer.
    let _manager = manager;
    let _device = device;

    let mut state = State {
        offers: HashMap::new(),
        entries: load_history(),
        pending: None,
        history: history.clone(),
        dirty: false,
    };

    // Publish whatever was persisted from previous sessions straight away.
    let _ = state.history.send(state.entries.clone());

    loop {
        if queue.blocking_dispatch(&mut state).is_err() {
            tracing::warn!("clipboard: Wayland connection closed");
            return;
        }

        // A selection arrived and told us which pipe to read.
        if let Some((offer, mime)) = state.pending.take() {
            if let Some(text) = read_offer(&offer, &mime, connection) {
                state.record(text);
            }
            offer.destroy();
        }

        if state.dirty {
            state.dirty = false;
            save_history(&state.entries);
            if state.history.send(state.entries.clone()).is_err() {
                return;
            }
        }
    }
}

/// Ask for the offer's contents and read them off the pipe.
///
/// The read happens on a scratch thread: the writing application controls how
/// fast the pipe fills, and a misbehaving one must not be able to wedge the
/// clipboard thread.
fn read_offer(
    offer: &ZwlrDataControlOfferV1,
    mime: &str,
    connection: &Connection,
) -> Option<String> {
    let Ok((reader, writer)) = std::io::pipe() else {
        return None;
    };

    offer.receive(mime.to_owned(), writer.as_fd());
    // The request must reach the compositor before the writer is dropped,
    // otherwise the source sees a closed pipe and writes nothing.
    let _ = connection.flush();
    drop(writer);

    let (tx, rx) = std_mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        // Bounded so a source offering something enormous cannot exhaust memory.
        let _ = reader.take(MAX_ENTRY as u64).read_to_end(&mut buffer);
        let _ = tx.send(buffer);
    });

    let buffer = rx.recv_timeout(READ_TIMEOUT).ok()?;
    let text = String::from_utf8(buffer).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| text.clone())
}

struct State {
    /// MIME types advertised by each offer, accumulated before `selection`.
    offers: HashMap<ZwlrDataControlOfferV1, Vec<String>>,
    entries: Vec<Entry>,
    /// Set when a selection is ready to be read, outside the dispatch callback.
    pending: Option<(ZwlrDataControlOfferV1, String)>,
    history: mpsc::UnboundedSender<Vec<Entry>>,
    dirty: bool,
}

impl State {
    fn record(&mut self, text: String) {
        let entry = Entry { text };

        // Copying the same thing twice should promote it, not duplicate it.
        self.entries.retain(|existing| existing != &entry);
        self.entries.insert(0, entry);
        self.entries.truncate(HISTORY_LIMIT);
        self.dirty = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // A new offer: its MIME types arrive next, on the offer itself.
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(id, Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                let Some(offer) = id else { return };
                let mimes = state.offers.remove(&offer).unwrap_or_default();

                // Pick the best text type the source actually advertised.
                let chosen = TEXT_MIMES
                    .iter()
                    .find(|candidate| mimes.iter().any(|mime| mime == *candidate));

                match chosen {
                    Some(mime) => state.pending = Some((offer, (*mime).to_owned())),
                    None => {
                        // Images and custom types are not stored; only text.
                        offer.destroy();
                    }
                }
            }
            // Primary selection (middle-click) is deliberately ignored: mixing
            // it into the same history makes the list unpredictable, because it
            // changes on every drag-select.
            zwlr_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                state.offers.remove(&offer);
                offer.destroy();
            }
            zwlr_data_control_device_v1::Event::Finished => {
                tracing::info!("clipboard: device finished");
            }
            _ => {}
        }
    }

    // `data_offer` creates a new protocol object, and wayland-client cannot
    // construct it without being told how. This must live inside the impl: at
    // module level it still compiles, then panics on the first selection.
    cosmic::cctk::wayland_client::event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            state
                .offers
                .entry(offer.clone())
                .or_default()
                .push(mime_type);
        }
    }
}

/// Forget everything in history, on disk as well as in memory.
///
/// The file is removed rather than truncated so nothing recoverable is left
/// behind — the point of the action is that the contents are gone.
pub fn clear_history() {
    if let Some(path) = history_path()
        && let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, ?path, "could not clear clipboard history");
    }
}

fn history_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|dir| dir.join("jump").join("clipboard.json"))
}

fn load_history() -> Vec<Entry> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&contents).unwrap_or_else(|error| {
        tracing::warn!(%error, "discarding unreadable clipboard history");
        Vec::new()
    })
}

fn save_history(entries: &[Entry]) {
    let Some(path) = history_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let Ok(contents) = serde_json::to_string(entries) else {
        return;
    };

    // Clipboard history routinely contains passwords and tokens, so the file is
    // written readable only by its owner. Write-then-rename keeps a crash from
    // truncating it.
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, contents).is_err() {
        return;
    }
    let _ = std::fs::set_permissions(
        &temporary,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    );
    let _ = std::fs::rename(&temporary, &path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_collapses_whitespace() {
        let entry = Entry {
            text: "hello\n\n   world\t!".to_owned(),
        };
        assert_eq!(entry.preview(), "hello world !");
    }

    #[test]
    fn preview_truncates_long_text() {
        let entry = Entry {
            text: "x".repeat(500),
        };
        assert!(entry.preview().chars().count() <= 121);
        assert!(entry.preview().ends_with('…'));
    }

    #[test]
    fn describe_reports_shape() {
        let single = Entry {
            text: "hello".to_owned(),
        };
        assert!(single.describe().contains("5 characters"));

        let multi = Entry {
            text: "a\nb\nc".to_owned(),
        };
        assert!(multi.describe().contains("3 lines"));
    }
}
