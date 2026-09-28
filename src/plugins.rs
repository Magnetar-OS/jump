// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Live plugin discovery, shared by the launcher and the settings window.
//!
//! Plugins are directories, installed by `jump plugin new`, `jump plugin
//! import`, a `git clone` or a package. Both processes run all session, so
//! discovering once at startup meant a new plugin did nothing until a
//! restart — while the CLI had just told the user to go and type its
//! keyword. The plugin roots are watched instead, and every change that can
//! alter what discovery finds produces a fresh [`PluginHost`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use cosmic::iced::futures::{SinkExt, Stream};
use jump_core::PluginHost;
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecursiveMode, Watcher};

/// How long a burst of changes is left to settle before rediscovering. A
/// `git clone` or an editor's save-by-rename is a dozen events for one
/// change, and discovering in the middle of it would read a half-written
/// manifest.
const SETTLE: Duration = Duration::from_millis(250);

/// Rediscover plugins whenever [`PluginHost::roots`] change.
///
/// The first item is a discovery made once the watches are in place, which
/// closes the gap between a caller's startup discovery and the watch.
pub fn watch() -> impl Stream<Item = PluginHost> {
    watch_in(PluginHost::roots())
}

/// [`watch`] over explicit roots, earlier roots shadowing later ones.
pub fn watch_in(roots: Vec<PathBuf>) -> impl Stream<Item = PluginHost> {
    cosmic::iced::stream::channel(4, async move |mut output| {
        // The user's root is created up front: a root that does not exist
        // cannot be watched, and the first `jump plugin new` would otherwise
        // create it unseen.
        if let Some(user) = roots.first()
            && let Err(error) = std::fs::create_dir_all(user)
        {
            tracing::warn!(%error, root = ?user, "cannot create the plugin directory");
        }

        let (signal, mut signals) = tokio::sync::mpsc::unbounded_channel();
        let relevant_roots = roots.clone();
        let watcher =
            notify::recommended_watcher(move |event: notify::Result<Event>| match event {
                Ok(event) if affects_discovery(&relevant_roots, &event) => {
                    // Fails only once the stream has ended and nobody is
                    // listening any more.
                    let _ = signal.send(());
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "plugin directory watch failed"),
            });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::warn!(%error, "cannot watch the plugin directories; new plugins need a restart");
                return;
            }
        };
        for root in roots.iter().filter(|root| root.is_dir()) {
            if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
                tracing::warn!(%error, ?root, "cannot watch a plugin directory");
            }
        }

        loop {
            let scan = roots.clone();
            // Discovery is blocking file I/O; keep it off the executor.
            let host =
                match tokio::task::spawn_blocking(move || PluginHost::discover_in(&scan)).await {
                    Ok(host) => host,
                    Err(error) => {
                        tracing::warn!(%error, "plugin discovery failed");
                        return;
                    }
                };
            if output.send(host).await.is_err() || signals.recv().await.is_none() {
                return;
            }
            tokio::time::sleep(SETTLE).await;
            while signals.try_recv().is_ok() {}
        }
    })
}

/// Whether `event` can change what [`PluginHost::discover_in`] finds under
/// `roots`.
///
/// Discovery reads a root's entries and each entry's `manifest.toml`, so only
/// those count. Everything else is filtered out, and has to be: discovery
/// itself opens every manifest, a plugin may keep a cache in its own
/// directory, and reacting to either would rediscover in a loop.
fn affects_discovery(roots: &[PathBuf], event: &Event) -> bool {
    let kind = matches!(
        event.kind,
        EventKind::Any
            | EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_))
    );
    kind && event.paths.iter().any(|path| {
        roots
            .iter()
            .filter_map(|root| path.strip_prefix(root).ok())
            .any(is_discovered_path)
    })
}

/// A plugin directory (`<id>`) or its manifest (`<id>/manifest.toml`),
/// relative to its root.
fn is_discovered_path(relative: &Path) -> bool {
    let mut components = relative.components();
    match (components.next(), components.next(), components.next()) {
        (Some(_), None, None) => true,
        (Some(_), Some(file), None) => file.as_os_str() == "manifest.toml",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::futures::StreamExt;
    use notify::event::{AccessKind, CreateKind, MetadataKind};

    fn event(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn only_plugin_directories_and_manifests_trigger_discovery() {
        let roots = [PathBuf::from("/p")];
        let create = EventKind::Create(CreateKind::Any);

        assert!(affects_discovery(&roots, &event(create, "/p/gh")));
        assert!(affects_discovery(
            &roots,
            &event(create, "/p/gh/manifest.toml")
        ));
        // A plugin's own files, deeper trees and other directories are not
        // what discovery reads.
        assert!(!affects_discovery(
            &roots,
            &event(create, "/p/gh/cache.json")
        ));
        assert!(!affects_discovery(
            &roots,
            &event(create, "/p/gh/lib/manifest.toml")
        ));
        assert!(!affects_discovery(&roots, &event(create, "/elsewhere/gh")));
        // Discovery opening a manifest must not trigger another discovery.
        let opened = EventKind::Access(AccessKind::Any);
        assert!(!affects_discovery(
            &roots,
            &event(opened, "/p/gh/manifest.toml")
        ));
        let touched = EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime));
        assert!(!affects_discovery(
            &roots,
            &event(touched, "/p/gh/manifest.toml")
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_plugin_installed_after_startup_is_discovered() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut hosts = Box::pin(watch_in(vec![root.path().to_path_buf()]));

        let initial = hosts.next().await.expect("an initial discovery");
        assert!(initial.is_empty());

        let plugin = root.path().join("hello");
        std::fs::create_dir(&plugin).expect("plugin dir");
        std::fs::write(
            plugin.join("manifest.toml"),
            "name = \"Hello\"\nkeyword = \"hello\"\nquery = \"./search.sh\"\n",
        )
        .expect("manifest");

        let found = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let host = hosts.next().await.expect("the watch ended");
                if host.get("hello").is_some() {
                    return host;
                }
            }
        })
        .await;
        assert!(found.is_ok(), "the new plugin was never discovered");
    }
}
