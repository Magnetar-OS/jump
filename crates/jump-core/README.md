# jump-core

Search engine, ranking, and plugin host for the [jump](https://github.com/Magnetar-OS/jump)
launcher.

This crate is everything a launcher frontend needs that is not drawing. It has
no UI dependencies, which is the point: the COSMIC frontend (libcosmic) and a
future GNOME frontend (GTK4) link the same engine instead of drifting into two
half-implementations. That split is also why this crate is MPL-2.0 while the
COSMIC application is GPL-3.0-only.

## What is in it

| Module | Does |
| --- | --- |
| `launcher` | Async bridge to the `pop-launcher` IPC service |
| `model` | Result model, and identity that survives re-querying |
| `rank` | Puts four providers' results on one comparable scale |
| `frecency` | Usage-weighted re-ranking, Mozilla's frecency shape |
| `files` | Private path index — the system `plocate` database is not usable |
| `content` | Full-text search inside files, on SQLite FTS5 |
| `plugin` | Alfred-style plugin host: manifest plus a program that prints JSON |
| `process` | Process search and termination, behind a `kill` keyword |
| `calc` | Calculator and unit conversion, over Qalculate's `qalc` |
| `emoji` | Emoji search over a compiled-in data set, no I/O |
| `web` | Quicklinks and fallback searches from URL templates |

Each module's own documentation explains why it exists and what it measured,
which is usually the more interesting half.

## Usage

```rust,no_run
use jump_core::{Event, Frecency, Launcher, PluginHost};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let (launcher, mut events, _guard) = Launcher::spawn()?;
let plugins = PluginHost::discover();
let frecency = Frecency::load();

launcher.search("fire");
while let Some(event) = events.recv().await {
    if let Event::Update { items, .. } = event {
        // One scale for every provider, then usage as a tiebreak.
        let ranked = jump_core::rank::merge(items, "fire", &frecency);
        let _ = ranked;
    }
}
# Ok(())
# }
```

## Runtime expectations

The engine shells out rather than linking heavyweight libraries, so some
providers need a program on `PATH` and degrade to returning nothing when it is
absent:

- `launcher` needs `pop-launcher`.
- `calc` needs `qalc` (Qalculate).
- `files` and `content` write their indexes under the XDG data directory.
- `process` reads `/proc`, so it is Linux-only.

## Stability

Pre-1.0: the API may change between minor versions. Note that `Item` and the
`Launcher` methods currently expose `pop_launcher` types (`Indice`,
`Generation`, `ContextOption`, `GpuPreference`) in the public API, so a
breaking release of `pop-launcher` forces one here.

## License

MPL-2.0. See [LICENSE](LICENSE).
