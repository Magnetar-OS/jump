// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Live window list and activation, over the COSMIC toplevel protocols.
//!
//! pop-launcher ships a `cosmic_toplevel` plugin that is supposed to provide
//! this, but on COSMIC 1.5 it returns nothing — verified by driving the plugin
//! binary directly and by querying the service, both of which produce zero
//! window results. Rather than depend on a plugin that does not work, the
//! switcher talks to `ext-foreign-toplevel-list` and
//! `zcosmic-toplevel-manager` itself through `cctk`, which libcosmic re-exports
//! at exactly the version the running compositor speaks.
//!
//! ## Why a thread
//!
//! iced/winit owns the application's Wayland connection and its event queue, and
//! there is no supported way to bind extra globals on it from application code.
//! A second connection on its own thread is the normal answer: it is a handful
//! of protocol objects, it blocks on its own queue without touching the render
//! loop, and it communicates over channels like any other backend.

use std::sync::mpsc as std_mpsc;

use cosmic::cctk::cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::{
    self, ZcosmicToplevelHandleV1,
};
use cosmic::cctk::cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1;
use cosmic::cctk::sctk::output::{OutputHandler, OutputState};
use cosmic::cctk::sctk::registry::{ProvidesRegistryState, RegistryState};
use cosmic::cctk::sctk::seat::{SeatHandler, SeatState};
use cosmic::cctk::toplevel_info::{ToplevelInfoHandler, ToplevelInfoState};
use cosmic::cctk::toplevel_management::{ToplevelManagerHandler, ToplevelManagerState};
use cosmic::cctk::wayland_client::globals::registry_queue_init;
use cosmic::cctk::wayland_client::protocol::{wl_output, wl_seat};
use cosmic::cctk::wayland_client::{Connection, QueueHandle};
use cosmic::cctk::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
use cosmic::cctk::sctk;
use tokio::sync::mpsc;

/// One open window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Compositor-assigned identifier, stable for the window's lifetime. Used
    /// both as the activation address and as the stable list key.
    pub identifier: String,
    pub title: String,
    pub app_id: String,
    /// Current compositor state, so the action panel can label Maximize
    /// against Restore truthfully instead of offering a blind toggle.
    pub maximized: bool,
    pub fullscreen: bool,
}

/// A request from the UI thread to the Wayland thread.
enum Request {
    /// Focus the window with this identifier.
    Activate(String),
    /// Ask the window with this identifier to close.
    Close(String),
    Maximize(String),
    Unmaximize(String),
    Minimize(String),
    Fullscreen(String),
    Unfullscreen(String),
}

/// Handle used by the application to drive the switcher.
#[derive(Debug, Clone)]
pub struct Toplevels {
    requests: std_mpsc::Sender<Request>,
}

impl Toplevels {
    /// Focus a window, raising it and switching workspace if needed.
    pub fn activate(&self, identifier: &str) {
        let _ = self.requests.send(Request::Activate(identifier.to_owned()));
    }

    /// Ask a window to close.
    pub fn close(&self, identifier: &str) {
        let _ = self.requests.send(Request::Close(identifier.to_owned()));
    }

    pub fn maximize(&self, identifier: &str) {
        let _ = self.requests.send(Request::Maximize(identifier.to_owned()));
    }

    pub fn unmaximize(&self, identifier: &str) {
        let _ = self
            .requests
            .send(Request::Unmaximize(identifier.to_owned()));
    }

    pub fn minimize(&self, identifier: &str) {
        let _ = self.requests.send(Request::Minimize(identifier.to_owned()));
    }

    pub fn fullscreen(&self, identifier: &str) {
        let _ = self
            .requests
            .send(Request::Fullscreen(identifier.to_owned()));
    }

    pub fn unfullscreen(&self, identifier: &str) {
        let _ = self
            .requests
            .send(Request::Unfullscreen(identifier.to_owned()));
    }
}

/// Start the Wayland thread.
///
/// Returns a handle for activation plus a stream of window-list snapshots. The
/// full list is resent on every change rather than a delta: a dozen windows is
/// nothing to clone, and a snapshot cannot desynchronise from the compositor the
/// way an incrementally-patched list can.
///
/// Returns `None` when the compositor does not implement the protocols, in which
/// case the switcher is simply unavailable.
pub fn spawn() -> Option<(Toplevels, mpsc::UnboundedReceiver<Vec<Window>>)> {
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(error) => {
            tracing::warn!(%error, "no Wayland connection for the window switcher");
            return None;
        }
    };

    let (windows_tx, windows_rx) = mpsc::unbounded_channel();
    let (request_tx, request_rx) = std_mpsc::channel();

    std::thread::Builder::new()
        .name("jump-toplevels".into())
        .spawn(move || run(&connection, &windows_tx, &request_rx))
        .ok()?;

    Some((
        Toplevels {
            requests: request_tx,
        },
        windows_rx,
    ))
}

fn run(
    connection: &Connection,
    windows: &mpsc::UnboundedSender<Vec<Window>>,
    requests: &std_mpsc::Receiver<Request>,
) {
    let Ok((globals, mut queue)) = registry_queue_init::<State>(connection) else {
        tracing::warn!("failed to initialise the Wayland registry for toplevels");
        return;
    };
    let qh = queue.handle();

    let registry_state = RegistryState::new(&globals);
    let Some(toplevel_info_state) = ToplevelInfoState::try_new(&registry_state, &qh) else {
        tracing::info!("compositor does not implement toplevel-info; switcher disabled");
        return;
    };
    let toplevel_manager_state = ToplevelManagerState::try_new(&registry_state, &qh);
    if toplevel_manager_state.is_none() {
        // The list still works; only activation is unavailable.
        tracing::info!("compositor does not implement zcosmic-toplevel-manager");
    }

    let mut state = State {
        // Outputs must be bound even though the switcher does not use them:
        // toplevel handles emit output-enter/leave events, and without the
        // global bound those events have no proxy to resolve against.
        output_state: OutputState::new(&globals, &qh),
        // Activation is addressed to a seat, so one has to be bound.
        seat_state: SeatState::new(&globals, &qh),
        toplevel_info_state,
        toplevel_manager_state,
        registry_state,
        windows: windows.clone(),
        dirty: false,
    };

    loop {
        // Drain activation requests before blocking, so a keypress is not held
        // until the next unrelated compositor event wakes the queue.
        while let Ok(request) = requests.try_recv() {
            state.handle(&request, &qh);
        }

        if queue.blocking_dispatch(&mut state).is_err() {
            tracing::warn!("toplevel Wayland connection closed");
            return;
        }

        if state.dirty {
            state.dirty = false;
            if state.publish().is_err() {
                // The application dropped the receiver; nothing left to do.
                return;
            }
        }
    }
}

struct State {
    output_state: OutputState,
    seat_state: SeatState,
    registry_state: RegistryState,
    toplevel_info_state: ToplevelInfoState,
    toplevel_manager_state: Option<ToplevelManagerState>,
    windows: mpsc::UnboundedSender<Vec<Window>>,
    /// Set when the toplevel set changed and a snapshot is owed.
    dirty: bool,
}

impl State {
    fn publish(&self) -> Result<(), mpsc::error::SendError<Vec<Window>>> {
        let windows = self
            .toplevel_info_state
            .toplevels()
            .map(|info| Window {
                identifier: info.identifier.clone(),
                title: info.title.clone(),
                app_id: info.app_id.clone(),
                maximized: info
                    .state
                    .contains(&zcosmic_toplevel_handle_v1::State::Maximized),
                fullscreen: info
                    .state
                    .contains(&zcosmic_toplevel_handle_v1::State::Fullscreen),
            })
            .collect();

        self.windows.send(windows)
    }

    /// Resolve an identifier back to the COSMIC handle that management requests
    /// are addressed to.
    ///
    /// The handle itself is never sent to the application: it is bound to this
    /// thread's connection and is not `Send`. `cosmic_toplevel` is `None` when
    /// the compositor only speaks the foreign-toplevel list, which is enough to
    /// show windows but not to act on them.
    fn handle_for(&self, identifier: &str) -> Option<ZcosmicToplevelHandleV1> {
        self.toplevel_info_state
            .toplevels()
            .find(|info| info.identifier == identifier)
            .and_then(|info| info.cosmic_toplevel.clone())
    }

    fn handle(&mut self, request: &Request, _qh: &QueueHandle<Self>) {
        let identifier = match request {
            Request::Activate(identifier)
            | Request::Close(identifier)
            | Request::Maximize(identifier)
            | Request::Unmaximize(identifier)
            | Request::Minimize(identifier)
            | Request::Fullscreen(identifier)
            | Request::Unfullscreen(identifier) => identifier,
        };

        let Some(handle) = self.handle_for(identifier) else {
            tracing::debug!(%identifier, "window vanished before the request was sent");
            return;
        };
        let Some(manager) = self.toplevel_manager_state.as_ref() else {
            return;
        };

        match request {
            Request::Activate(_) => {
                // Activation is addressed to a seat. The first is the
                // pointer/keyboard seat on any single-seat setup.
                let Some(seat) = self.seat_state.seats().next() else {
                    tracing::warn!("no seat available to activate a window");
                    return;
                };
                manager.manager.activate(&handle, &seat);
            }
            Request::Close(_) => manager.manager.close(&handle),
            Request::Maximize(_) => manager.manager.set_maximized(&handle),
            Request::Unmaximize(_) => manager.manager.unset_maximized(&handle),
            Request::Minimize(_) => manager.manager.set_minimized(&handle),
            // The compositor picks the output when none is named.
            Request::Fullscreen(_) => manager.manager.set_fullscreen(&handle, None),
            Request::Unfullscreen(_) => manager.manager.unset_fullscreen(&handle),
        }
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    sctk::registry_handlers!(OutputState, SeatState);
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: sctk::seat::Capability,
    ) {
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: sctk::seat::Capability,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ToplevelInfoHandler for State {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.toplevel_info_state
    }

    fn new_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ExtForeignToplevelHandleV1,
    ) {
        self.dirty = true;
    }

    fn update_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ExtForeignToplevelHandleV1,
    ) {
        self.dirty = true;
    }

    fn toplevel_closed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ExtForeignToplevelHandleV1,
    ) {
        self.dirty = true;
    }

    fn info_done(&mut self, _: &Connection, _: &QueueHandle<Self>) {
        self.dirty = true;
    }
}

impl ToplevelManagerHandler for State {
    fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState {
        self.toplevel_manager_state
            .as_mut()
            .expect("manager events only arrive when the manager is bound")
    }

    fn capabilities(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: Vec<cosmic::cctk::wayland_client::WEnum<ZcosmicToplelevelManagementCapabilitiesV1>>,
    ) {
    }
}

cosmic::cctk::delegate_toplevel_info!(State);
cosmic::cctk::delegate_toplevel_manager!(State);
sctk::delegate_output!(State);
sctk::delegate_seat!(State);
sctk::delegate_registry!(State);
