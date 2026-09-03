// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Bluetooth devices and Wi-Fi networks as launcher results.
//!
//! Both are the same shape of problem: a list that only the system bus knows,
//! addressed by an object path, acted on with one method call. And both have
//! the same constraint — a bus round trip must never happen on the match path,
//! which runs synchronously for every keystroke.
//!
//! So the list is a *snapshot*, fetched once when the overlay opens (the same
//! pattern the media commands' "what's playing" uses) and matched against
//! synchronously afterwards. A device that appears while the launcher is open
//! is missed until the next open, which is the right trade: the alternative is
//! subscribing to two daemons' property changes for the whole session to keep
//! a list that is looked at for a few seconds at a time.
//!
//! Only *paired* Bluetooth devices and *saved* Wi-Fi networks are offered.
//! Connecting to something unpaired or unconfigured needs an agent, a PIN and
//! a dialog — that is COSMIC Settings' job, and the launcher deep-links there
//! through the existing system commands.

use jump_core::{Icon, Item, ItemKey, Source};

use jump::fl;

/// Prefixes that route a [`Source::System`] id back here. They are part of the
/// stored frecency key, so they are stable identifiers, not display strings.
const BT_CONNECT: &str = "bt-connect:";
const BT_DISCONNECT: &str = "bt-disconnect:";
const WIFI_CONNECT: &str = "wifi-connect:";
const WIFI_DISCONNECT: &str = "wifi-disconnect:";

/// Commands only surface once this much of the query matches, matching the
/// built-in system commands' threshold.
const MIN_SCORE: f32 = 0.45;

/// One thing the user can connect to or disconnect from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Device {
    /// A paired Bluetooth device, addressed by its bluez object path.
    Bluetooth {
        path: String,
        name: String,
        connected: bool,
    },
    /// A saved Wi-Fi network, addressed by its NetworkManager *settings*
    /// path. `active` carries the active-connection path when it is the
    /// network currently in use, which is what deactivation is addressed to.
    Wifi {
        path: String,
        name: String,
        active: Option<String>,
    },
}

/// What activating a device result does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    BluetoothConnect(String),
    BluetoothDisconnect(String),
    /// Activate a saved connection, by settings path.
    WifiConnect(String),
    /// Deactivate an active connection, by active-connection path.
    WifiDisconnect(String),
}

/// Decode a [`Source::System`] id back into an action, or `None` when the id
/// belongs to one of the built-in commands instead.
#[must_use]
pub fn action_for(id: &str) -> Option<Action> {
    if let Some(path) = id.strip_prefix(BT_CONNECT) {
        Some(Action::BluetoothConnect(path.to_owned()))
    } else if let Some(path) = id.strip_prefix(BT_DISCONNECT) {
        Some(Action::BluetoothDisconnect(path.to_owned()))
    } else if let Some(path) = id.strip_prefix(WIFI_CONNECT) {
        Some(Action::WifiConnect(path.to_owned()))
    } else {
        id.strip_prefix(WIFI_DISCONNECT)
            .map(|path| Action::WifiDisconnect(path.to_owned()))
    }
}

/// How well `query` matches a device, in `0.0..=1.0`.
///
/// Every token has to land somewhere — the device's own name or the kind's
/// vocabulary — so "bluetooth headphones" matches a pair of headphones but
/// "bluetooth firefox" matches nothing.
fn match_score(name: &str, keywords: &[&str], tokens: &[String]) -> f32 {
    if tokens.is_empty() {
        return 0.0;
    }
    let name = name.to_lowercase();
    let mut total = 0.0;

    for token in tokens {
        // Below three characters everything prefix-matches something; real
        // matches arrive a keystroke later.
        if token.chars().count() < 3 {
            return 0.0;
        }
        if name.contains(token.as_str()) {
            total += 1.0;
        } else if keywords
            .iter()
            .any(|keyword| keyword.starts_with(token.as_str()))
        {
            total += 0.8;
        } else {
            return 0.0;
        }
    }

    total / tokens.len() as f32
}

/// Device results matching `query`, pre-scored and ready to merge.
#[must_use]
pub fn matching(query: &str, devices: &[Device]) -> Vec<Item> {
    let tokens: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();

    devices
        .iter()
        .filter_map(|device| {
            let (name, keywords, id, title, subtitle, icon) = match device {
                Device::Bluetooth {
                    path,
                    name,
                    connected: true,
                } => (
                    name,
                    &["bluetooth", "disconnect", "device"][..],
                    format!("{BT_DISCONNECT}{path}"),
                    fl!("device-disconnect", name = name.as_str()),
                    fl!("device-bluetooth-connected"),
                    "bluetooth-active-symbolic",
                ),
                Device::Bluetooth { path, name, .. } => (
                    name,
                    &["bluetooth", "connect", "pair", "device"][..],
                    format!("{BT_CONNECT}{path}"),
                    fl!("device-connect", name = name.as_str()),
                    fl!("device-bluetooth"),
                    "bluetooth-symbolic",
                ),
                Device::Wifi {
                    name,
                    active: Some(active),
                    ..
                } => (
                    name,
                    &["wifi", "wireless", "network", "disconnect"][..],
                    format!("{WIFI_DISCONNECT}{active}"),
                    fl!("device-disconnect", name = name.as_str()),
                    fl!("device-wifi-connected"),
                    "network-wireless-symbolic",
                ),
                Device::Wifi { path, name, .. } => (
                    name,
                    &["wifi", "wireless", "network", "connect"][..],
                    format!("{WIFI_CONNECT}{path}"),
                    fl!("device-connect", name = name.as_str()),
                    fl!("device-wifi"),
                    "network-wireless-symbolic",
                ),
            };

            let score = match_score(name, keywords, &tokens);
            if score < MIN_SCORE {
                return None;
            }

            Some(Item {
                key: ItemKey(format!("system:{id}")),
                id: 0,
                title,
                subtitle,
                icon: Some(Icon::Name(icon.to_owned())),
                category_icon: None,
                window: None,
                source: Source::System { id },
                autocomplete: None,
                score,
            })
        })
        .collect()
}

/// Fetch the current device list from both daemons.
///
/// Either daemon being absent costs its half of the list and nothing else —
/// a machine without Bluetooth is not an error state.
pub async fn snapshot() -> Vec<Device> {
    // Both daemons are asked at once: the two round trips overlap instead of
    // adding up.
    let (bluetooth, wifi) = tokio::join!(bluetooth_devices(), wifi_networks());
    let mut devices = bluetooth;
    devices.extend(wifi);
    devices
}

/// Paired Bluetooth devices, from bluez's object manager.
async fn bluetooth_devices() -> Vec<Device> {
    match fetch_bluetooth().await {
        Ok(devices) => devices,
        Err(error) => {
            tracing::debug!(%error, "bluez unavailable; no Bluetooth results");
            Vec::new()
        }
    }
}

async fn fetch_bluetooth() -> zbus::Result<Vec<Device>> {
    let connection = zbus::Connection::system().await?;
    let manager = zbus::fdo::ObjectManagerProxy::builder(&connection)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?;

    let objects = manager.get_managed_objects().await?;
    let mut devices = Vec::new();

    for (path, interfaces) in objects {
        let Some(properties) = interfaces.get("org.bluez.Device1") else {
            continue;
        };
        let read_bool = |key: &str| {
            properties
                .get(key)
                .and_then(|value| bool::try_from(value.try_clone().ok()?).ok())
                .unwrap_or(false)
        };
        let paired = read_bool("Paired");
        let connected = read_bool("Connected");
        // Unpaired devices need an agent and a PIN dialog to connect, which is
        // COSMIC Settings' job, so they are not offered here.
        if !paired && !connected {
            continue;
        }

        let name = properties
            .get("Alias")
            .or_else(|| properties.get("Name"))
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .filter(|name| !name.is_empty());
        let Some(name) = name else { continue };

        devices.push(Device::Bluetooth {
            path: path.to_string(),
            name,
            connected,
        });
    }

    devices.sort_by(|a, b| match (a, b) {
        (Device::Bluetooth { name: a, .. }, Device::Bluetooth { name: b, .. }) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    });
    Ok(devices)
}

/// Saved Wi-Fi networks, from NetworkManager's settings.
async fn wifi_networks() -> Vec<Device> {
    match fetch_wifi().await {
        Ok(networks) => networks,
        Err(error) => {
            tracing::debug!(%error, "NetworkManager unavailable; no Wi-Fi results");
            Vec::new()
        }
    }
}

async fn fetch_wifi() -> zbus::Result<Vec<Device>> {
    const NM: &str = "org.freedesktop.NetworkManager";
    let connection = zbus::Connection::system().await?;

    // Map every *saved* connection path to its active-connection path, when
    // it is the one currently in use. Deactivation is addressed to the active
    // path, activation to the saved one, so both are needed.
    let manager = zbus::Proxy::new(&connection, NM, "/org/freedesktop/NetworkManager", NM).await?;
    let active_paths: Vec<zbus::zvariant::OwnedObjectPath> = manager
        .get_property("ActiveConnections")
        .await
        .unwrap_or_default();

    let mut active_for: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for active in active_paths {
        let proxy = zbus::Proxy::new(
            &connection,
            NM,
            active.as_str().to_owned(),
            "org.freedesktop.NetworkManager.Connection.Active",
        )
        .await?;
        if let Ok(settings) = proxy
            .get_property::<zbus::zvariant::OwnedObjectPath>("Connection")
            .await
        {
            active_for.insert(settings.to_string(), active.to_string());
        }
    }

    let settings = zbus::Proxy::new(
        &connection,
        NM,
        "/org/freedesktop/NetworkManager/Settings",
        "org.freedesktop.NetworkManager.Settings",
    )
    .await?;
    let saved: Vec<zbus::zvariant::OwnedObjectPath> = settings.call("ListConnections", &()).await?;

    let mut networks = Vec::new();
    for path in saved {
        let proxy = zbus::Proxy::new(
            &connection,
            NM,
            path.as_str().to_owned(),
            "org.freedesktop.NetworkManager.Settings.Connection",
        )
        .await?;
        let Ok(config) = proxy
            .call::<_, _, std::collections::HashMap<
                String,
                std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
            >>("GetSettings", &())
            .await
        else {
            continue;
        };

        let Some(section) = config.get("connection") else {
            continue;
        };
        let string = |key: &str| {
            section
                .get(key)
                .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
        };
        if string("type").as_deref() != Some("802-11-wireless") {
            continue;
        }
        let Some(name) = string("id").filter(|name| !name.is_empty()) else {
            continue;
        };

        let path = path.to_string();
        let active = active_for.get(&path).cloned();
        networks.push(Device::Wifi { path, name, active });
    }

    networks.sort_by(|a, b| match (a, b) {
        (Device::Wifi { name: a, .. }, Device::Wifi { name: b, .. }) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    });
    Ok(networks)
}

/// Run a device action to completion.
///
/// Failures are logged rather than surfaced: by the time this runs the
/// launcher has dismissed and there is no UI left to show an error in.
pub async fn run(action: Action) {
    let result = match &action {
        Action::BluetoothConnect(path) => bluetooth_call(path, "Connect").await,
        Action::BluetoothDisconnect(path) => bluetooth_call(path, "Disconnect").await,
        Action::WifiConnect(path) => wifi_activate(path).await,
        Action::WifiDisconnect(path) => wifi_deactivate(path).await,
    };
    if let Err(error) = result {
        tracing::warn!(?action, %error, "device command failed");
    }
}

async fn bluetooth_call(path: &str, method: &str) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.bluez",
        path.to_owned(),
        "org.bluez.Device1",
    )
    .await?;
    // Connecting negotiates with a real radio and a real device, which takes
    // seconds; the launcher is already gone by then.
    proxy.call_method(method, &()).await?;
    Ok(())
}

async fn wifi_activate(settings: &str) -> zbus::Result<()> {
    const NM: &str = "org.freedesktop.NetworkManager";
    let connection = zbus::Connection::system().await?;
    let manager = zbus::Proxy::new(&connection, NM, "/org/freedesktop/NetworkManager", NM).await?;

    let settings = zbus::zvariant::ObjectPath::try_from(settings).map_err(zbus::Error::Variant)?;
    // "/" for the device and the specific object lets NetworkManager pick a
    // suitable radio itself, which is what `nmcli connection up` does.
    let any = zbus::zvariant::ObjectPath::try_from("/").expect("root path is valid");
    let _: zbus::zvariant::OwnedObjectPath = manager
        .call("ActivateConnection", &(&settings, &any, &any))
        .await?;
    Ok(())
}

async fn wifi_deactivate(active: &str) -> zbus::Result<()> {
    const NM: &str = "org.freedesktop.NetworkManager";
    let connection = zbus::Connection::system().await?;
    let manager = zbus::Proxy::new(&connection, NM, "/org/freedesktop/NetworkManager", NM).await?;

    let active = zbus::zvariant::ObjectPath::try_from(active).map_err(zbus::Error::Variant)?;
    manager
        .call_method("DeactivateConnection", &(&active,))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<Device> {
        vec![
            Device::Bluetooth {
                path: "/org/bluez/hci0/dev_AA".to_owned(),
                name: "EDIFIER Headphones".to_owned(),
                connected: false,
            },
            Device::Bluetooth {
                path: "/org/bluez/hci0/dev_BB".to_owned(),
                name: "Magic Mouse".to_owned(),
                connected: true,
            },
            Device::Wifi {
                path: "/org/freedesktop/NetworkManager/Settings/3".to_owned(),
                name: "Home Fibre".to_owned(),
                active: None,
            },
            Device::Wifi {
                path: "/org/freedesktop/NetworkManager/Settings/4".to_owned(),
                name: "Office".to_owned(),
                active: Some("/org/freedesktop/NetworkManager/ActiveConnection/2".to_owned()),
            },
        ]
    }

    #[test]
    fn devices_match_by_their_own_name() {
        let items = matching("edifier", &devices());
        assert_eq!(items.len(), 1);
        assert!(items[0].title.contains("EDIFIER"));
    }

    #[test]
    fn kind_vocabulary_lists_every_device_of_that_kind() {
        let bluetooth = matching("bluetooth", &devices());
        assert_eq!(bluetooth.len(), 2);

        let wifi = matching("wifi", &devices());
        assert_eq!(wifi.len(), 2);
    }

    #[test]
    fn connected_devices_offer_disconnect_and_address_the_right_path() {
        let items = matching("magic", &devices());
        let Source::System { id } = &items[0].source else {
            panic!("device results are system commands");
        };
        assert_eq!(
            action_for(id),
            Some(Action::BluetoothDisconnect(
                "/org/bluez/hci0/dev_BB".to_owned()
            ))
        );

        // An active Wi-Fi network deactivates by its *active* path, not the
        // saved-connection path activation uses.
        let office = matching("office", &devices());
        let Source::System { id } = &office[0].source else {
            panic!("device results are system commands");
        };
        assert_eq!(
            action_for(id),
            Some(Action::WifiDisconnect(
                "/org/freedesktop/NetworkManager/ActiveConnection/2".to_owned()
            ))
        );
    }

    #[test]
    fn inactive_networks_connect_by_settings_path() {
        let items = matching("home", &devices());
        let Source::System { id } = &items[0].source else {
            panic!("device results are system commands");
        };
        assert_eq!(
            action_for(id),
            Some(Action::WifiConnect(
                "/org/freedesktop/NetworkManager/Settings/3".to_owned()
            ))
        );
    }

    #[test]
    fn unrelated_and_short_queries_match_nothing() {
        assert!(matching("firefox", &devices()).is_empty());
        assert!(matching("bluetooth firefox", &devices()).is_empty());
        assert!(matching("ed", &devices()).is_empty());
        assert!(matching("", &devices()).is_empty());
    }

    /// Live-bus smoke test, against whatever bluez and NetworkManager
    /// actually report on this machine. Ignored by default because CI has
    /// neither daemon; run it on a real session with:
    ///
    /// ```sh
    /// cargo test --bin jump -- --ignored --nocapture live_buses
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs a live system bus with bluez and NetworkManager"]
    async fn snapshot_reads_the_live_buses() {
        let devices = snapshot().await;
        for device in &devices {
            match device {
                Device::Bluetooth {
                    path,
                    name,
                    connected,
                } => {
                    println!("bluetooth {name:?} connected={connected} at {path}");
                    assert!(path.starts_with("/org/bluez/"));
                }
                Device::Wifi { path, name, active } => {
                    println!("wifi {name:?} active={active:?} at {path}");
                    assert!(path.starts_with("/org/freedesktop/NetworkManager/Settings/"));
                }
            }
            assert!(!matches!(device, Device::Bluetooth { name, .. } if name.is_empty()));
        }
        // Every device must round-trip through the id encoding the results use.
        for item in matching("bluetooth", &devices)
            .into_iter()
            .chain(matching("wifi", &devices))
        {
            let Source::System { id } = &item.source else {
                panic!("device results are system commands");
            };
            assert!(action_for(id).is_some(), "id {id} does not decode");
        }
    }

    #[test]
    fn built_in_command_ids_are_not_device_actions() {
        assert_eq!(action_for("dark-mode"), None);
        assert_eq!(action_for("settings-network"), None);
    }
}
