// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Shared pieces of the `jump` frontend.
//!
//! The launcher, the panel applet, and the settings window are three binaries
//! that must agree on the application id and on the shape of the configuration.
//! They are kept here rather than duplicated: the settings window writes the
//! same `cosmic-config` store the launcher reads, and a divergence between them
//! would show up as settings that silently do nothing.

pub mod config;
pub mod localize;

/// Reverse-DNS identifier, used for the D-Bus name and the config store.
pub const APP_ID: &str = "com.magnetaros.Jump";

/// D-Bus object path the launcher serves its activation interface on.
///
/// libcosmic derives this from the application id, so it is spelled out here
/// once for the applet, which activates the launcher rather than spawning it.
pub const DBUS_PATH: &str = "/com/magnetaros/Jump";

/// Interface implemented by libcosmic's single-instance support.
pub const DBUS_ACTIVATION: &str = "org.freedesktop.DbusActivation";
