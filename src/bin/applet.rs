// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! COSMIC panel applet: a button that opens the launcher.
//!
//! Deliberately not a popup applet. Everything the launcher does already happens
//! in a full-screen overlay with its own keyboard focus, so reproducing any of
//! it inside a panel popup would be a second, worse copy of the same UI. The
//! applet's whole job is to be a click target for people who do not want to
//! remember a keyboard shortcut.
//!
//! Activation goes over D-Bus rather than by spawning `jump`: libcosmic's
//! single-instance support already serves `org.freedesktop.DbusActivation`, and
//! calling it directly avoids a process spawn on every click. Spawning is the
//! fallback for the case where the daemon is not running yet.

use std::collections::HashMap;

use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::{Element, widget};

const ID: &str = "dev.entro314labs.JumpApplet";

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,jump_applet=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    cosmic::applet::run::<Applet>(())
}

struct Applet {
    core: Core,
}

#[derive(Debug, Clone)]
enum Message {
    /// The panel button was pressed.
    Open,
    /// Nothing to do; activation is fire-and-forget.
    Done,
}

impl cosmic::Application for Applet {
    // An applet is one small process and the panel starts one per applet, so
    // the multi-threaded executor buys nothing here.
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, (): Self::Flags) -> (Self, Task<Self::Message>) {
        (Self { core }, Task::none())
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Done => Task::none(),
            Message::Open => Task::future(async {
                activate().await;
                cosmic::action::app(Message::Done)
            }),
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        // The panel recolours symbolic icons for the current theme, so the
        // launcher's own coloured icon would look out of place next to every
        // other applet.
        self.core
            .applet
            .icon_button_from_handle(widget::icon::from_name("system-search-symbolic").into())
            .on_press(Message::Open)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        // No popup: the launcher is its own surface.
        widget::text("").into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        // The panel supplies the transparent background an applet button needs;
        // without this the button sits on an opaque rectangle.
        Some(cosmic::applet::style())
    }
}

/// Ask the running launcher to show itself, starting it if it is not up.
async fn activate() {
    match try_activate().await {
        Ok(()) => {}
        Err(error) => {
            tracing::info!(%error, "launcher not running; starting it");
            if let Err(error) = std::process::Command::new("jump").spawn() {
                tracing::error!(%error, "could not start the launcher");
            }
        }
    }
}

async fn try_activate() -> Result<(), zbus::Error> {
    let connection = zbus::Connection::session().await?;

    // An empty platform-data map: the launcher takes no startup token from us,
    // because the surface it maps is a layer shell surface rather than a window
    // that would need activation.
    let platform_data: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();

    connection
        .call_method(
            Some(jump::APP_ID),
            jump::DBUS_PATH,
            Some(jump::DBUS_ACTIVATION),
            "Activate",
            &(platform_data,),
        )
        .await?;

    Ok(())
}
