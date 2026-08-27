# jump

A Spotlight-class launcher for the COSMIC desktop.

`jump` is a `wlr-layer-shell` overlay built on libcosmic 1.0 / iced 0.14. It
reuses the `pop-launcher` service as its search engine and adds the parts
COSMIC's built-in launcher does not have: an animated entrance, a result list
that transitions rather than snapping, usage-weighted ranking, compositor-side
backdrop blur, and an Alfred-style plugin format.

## Status

Working and dogfoodable on COSMIC 1.5. Verified on a live session:

- Overlay maps on the overlay layer above the panel and fullscreen windows
- Exclusive keyboard focus, typing, arrow/Ctrl-N/P navigation, Enter, Escape
- Live results from every pop-launcher plugin (apps, calc, files, recent,
  terminal, web)
- Launchpad: full-screen or panel, paginated or continuous
- Live window switching over `ext-foreign-toplevel-list`, merged into search
- File search over a private index, with cross-source ranking
- Clipboard history over `wlr-data-control`, behind the `clip` keyword
- Full-text search inside files (SQLite FTS5), opt-in
- Status-area icon via `StatusNotifierItem`, with a Settings item
- COSMIC panel applet, and a native settings window
- Compositor backdrop blur via `ext-background-effect-v1`
- Alfred-style plugins with `uid`/`autocomplete`/timeout support, verified
  end-to-end with the example below; per-plugin enable/disable in settings
- Built-in system commands: dark/light mode toggle and deep links into COSMIC
  Settings pages ("wifi" opens the network page)
- Action panel on Ctrl+K: reveal a file, copy its path, trash it, close a
  window, run an application's desktop actions
- Media control (play/pause, next, previous) with the current track shown,
  and power-profile switching
- Process search and kill behind the `kill` keyword; emoji search behind
  `emoji`, Enter copies
- Quicklinks (keyworded URL templates) and fallback web-search rows below
  every search's results
- `jump show <query>` opens with the query pre-filled, so a COSMIC custom
  shortcut can deep-link into e.g. clipboard history
- Process search behind the `kill` keyword — SIGTERM on Enter, Force Kill in
  the action panel

Not built yet: the GTK4/GNOME frontend.

## File search

`jump` builds and owns its own `plocate` database rather than using the system
one. That is not preference — the system database on a btrfs machine running
snapper held **37,153,401 paths of which 36,496,775 were under `/.snapshots/`**.
98.2% of it was snapshot duplicates, and every one of the first 2000 results for
`config` came from a snapshot, so post-filtering returns nothing. A private index
of `$HOME` takes **8.75 s to build, 44 MB on disk, 1.5 M entries**, and excludes
build trees by default.

Ranking is two-phase, because relevance is not something `plocate` provides — it
answers in database order. Phase one scores every candidate on strings alone (no
syscalls): where the query lands in the basename, path depth, hidden-path and
build-residue penalties, directory and extension priors. Phase two `stat`s only
the survivors to add recency. Over-fetch 2000, score, `stat` 120, return 12.

Results from different providers are then merged onto **one scale** by
[`jump_core::rank`](crates/jump-core/src/rank.rs). Concatenating provider lists
and sorting by position produced two real failures: pop-launcher returns a fixed
eight applications whether or not they match (querying `Cargo.toml` surfaced
*Alpha Protocol*), and file results, arriving milliseconds later, were appended
below them and fell off the panel. Every item now scores on comparable evidence —
chiefly whether what you typed appears in the title — with a per-source cap so no
single provider can fill the list.

## Content search

Off by default (`files.content`). Names are cheap to index; contents are not, and
a launcher that silently starts reading every document in a home directory is
doing something the user did not ask for.

The content index is a separate SQLite FTS5 database, not part of the path index,
because the two have opposite lifecycles: paths are cheap to rebuild wholesale on
a timer, contents must be incremental — only files whose `(mtime, size)` changed
are re-extracted. Ranking is BM25, results carry the matching excerpt, and the
index is queryable *while* it is still being built.

Extraction is plain text only. Formats needing external tools (PDF, office) are
out of scope: shelling out to `pdftotext` means inheriting its failure modes.

Indexing is bounded and prioritised. Candidates are ordered — the user's
documents and projects first, hidden trees last — before any extraction happens,
so the `content_max_mb` ceiling costs the least useful documents rather than
whichever happened to come last. Extraction reuses one buffer across the whole
pass and borrows the bytes when they are already valid UTF-8, instead of the
three allocations per file the first version made. The pass runs under
`block_in_place`, taking the index lock a chunk at a time, so queries interleave
with indexing rather than queueing behind it.

Package-manager caches are excluded from both indexes. On this machine they
contributed the majority of 41,412 content candidates and a 111 MB index made
entirely of other people's source code — excluding them cut the path index build
from 8.0 s to 2.1 s and was the single largest quality win available.

## Clipboard

`clip` shows history; `clip <text>` filters it. Behind a keyword deliberately:
clipboard history contains passwords and tokens, and putting those next to
application results, where a stray Enter pastes them somewhere, is a bad default.
History is capped at 200 entries, stored mode `0600`, and only text is kept.

## Modes

An empty query is **Launchpad** — every installed application, full screen by
default. Typing switches to **search**. Windows that match the query are listed
above launchables, because if a document is already open the user asking for it
by name wants to switch to it rather than open a second copy. `Ctrl+W` closes the
highlighted window without leaving the launcher.

## Configuration

Settings are stored through **cosmic-config**, the same mechanism every COSMIC
application uses, under `~/.config/cosmic/dev.entro314labs.Jump/v1/`. One RON
file per key, and changes are applied **live** — the daemon picks them up without
a restart, which matters for a process that stays running all session.

```sh
echo -n 'panel' > ~/.config/cosmic/dev.entro314labs.Jump/v1/grid_layout
echo -n 'false' > ~/.config/cosmic/dev.entro314labs.Jump/v1/blur
```

Full key reference: [`examples/settings.md`](examples/settings.md). An unset key
uses its default; an unreadable one logs and falls back, so a bad value cannot
stop the launcher opening.

## Why it is built this way

**The search engine is not ours.** `pop-launcher` already indexes desktop
entries, runs a calculator, searches files, and enumerates windows, and it
answers a warm query in **0.5–1.5 ms** — measured, not assumed. Rebuilding that
would have bought nothing. `jump` spends its effort on the frontend, which is
where the built-in launcher actually falls short.

**The overlay is full-screen, the panel is drawn inside it.** A layer surface
cannot be transformed by its client, and resizing it per frame would round-trip
to the compositor for a buffer configure. Drawing a panel inside a full-screen
surface means the entrance animation, the click-away region, and the blur
rectangle are all just surface-local geometry.

**Results have stable identity.** pop-launcher addresses results by an index
that is only valid for the query that produced it — "Firefox" can be id 0 for
`fire` and id 3 for `fi`. Keying the list on that index forces a full rebuild on
every keystroke, which is why the built-in launcher swaps its list instantly
instead of transitioning. `jump` derives an `ItemKey` that survives re-querying,
so the view can diff two result sets and animate the difference, and the
highlighted row stays on the item the user was about to hit.

**It is a daemon.** Spawning the service, indexing desktop entries, discovering
plugins, and creating a wgpu device all happen once at startup. A second
invocation reaches the running instance over D-Bus and only maps a surface.

## Install

Requires a Rust toolchain, [`just`](https://github.com/casey/just), and at
runtime: `pop-launcher` (ships with COSMIC), `plocate` for file search, and
`xdg-utils` for opening files.

```sh
just build-release
just install-user          # or: sudo just install
systemctl --user enable --now jump.service
```

Then bind a shortcut to `jump` in **Settings → Keyboard → Shortcuts**. Running
the binary while the daemon is up reaches it over D-Bus and toggles the overlay,
so the same command both starts and shows it.

`just uninstall-user` removes everything, including the service.

### Packaging

`just vendor` produces `vendor.tar` and `build-vendored` builds from it offline.
This is not optional for distribution packaging: libcosmic is a **git**
dependency, so a packaged build cannot resolve it from crates.io — which is also
why this cannot be published to crates.io as-is.

Debug logs go to stderr (`RUST_LOG=jump=debug`), where they cannot corrupt the
launcher service's JSON stream on stdout.

## Plugins

Two plugin systems work at once.

**pop-launcher plugins** are picked up automatically — anything already
installed under `/usr/lib/pop-launcher/plugins` or
`~/.local/share/pop-launcher/plugins` appears in results with no extra work.

**jump plugins** are for the cases where implementing pop-launcher's stateful
IPC protocol is more than a shell script should have to do. They are
discovered in `~/.local/share/jump/plugins` and in every `<dir>/jump/plugins`
on `$XDG_DATA_DIRS` (so a distribution can package one into
`/usr/share/jump/plugins`; a user plugin shadows a packaged plugin with the
same directory name). A manifest plus an executable that takes the query on
argv and prints JSON:

```
~/.local/share/jump/plugins/github/
  manifest.toml
  search.sh
```

```toml
name = "GitHub"
description = "Search your repositories"
keyword = "gh"          # optional; omit to run on every query
query = "./search.sh"   # invoked as: search.sh <text>
activate = "./open.sh"  # invoked as: open.sh <arg>
icon = "github"         # optional icon-theme name or path
timeout_ms = 1000       # optional; up to 3000 for slow keyworded plugins
```

The query program prints a subset of Alfred's Script Filter schema, so many
existing workflow scripts port by writing a manifest:

```json
{"items": [
  {"uid": "repo-jump", "title": "entro314-labs/jump", "subtitle": "Rust",
   "arg": "…", "autocomplete": "gh jump ", "valid": true}
]}
```

`uid` gives an item a stable identity, which is what lets the launcher's
usage-weighted ranking learn plugin items the way it learns applications.
`autocomplete` is what Tab replaces the query with. `variables` — top-level
or per-item, the item's winning — are exported into the activation command's
environment, Alfred-style, so state crosses from query to activation without
being packed into `arg`. A response carrying `rerun` (seconds, clamped
0.5–5.0) has its query re-run on that interval while it is on screen, which
is how a polling plugin streams updates. Individual plugins can be switched
off from the settings window.

A keyworded plugin takes the query over entirely: typing `gh jump` addresses
that plugin and app results are suppressed. The keyword must be followed by a
space or end the query, so `ghost` is not read as `gh` + `ost`.

Plugins are held to a **180 ms** deadline and run concurrently. A plugin that
overruns is killed and its results dropped — a broken plugin degrades its own
results and nothing else. Network-backed plugins are expected to cache.

A working example lives in [`examples/plugins/demo/`](examples/plugins/demo/).
Copy it to `~/.local/share/jump/plugins/demo/` and type `demo hello`.

`jump plugin new <name>` scaffolds a runnable plugin into the user plugin
directory, and `jump plugin lint <dir-or-name> [sample-query]` checks the
manifest, the commands it names, and one sample query's output against the
schema — by running it, under the same deadline the launcher uses.

## Layout

```
crates/jump-core/     MPL-2.0 — engine, no UI dependencies
  launcher.rs          pop-launcher IPC bridge
  model.rs             result model and stable identity
  frecency.rs          usage-weighted re-ranking
  plugin.rs            Alfred-style plugin host
src/                   GPL-3.0-only — the COSMIC frontend
  surface.rs           layer-shell surface, geometry, blur region
  anim.rs              open transition and row cascade
  view.rs              rendering
  app.rs               state and update loop
```

The engine is a separate MPL-2.0 crate specifically so a future GTK4/GNOME
frontend can link it without absorbing the application's licence.

## GNOME

Mutter does not implement `wlr-layer-shell` and has repeatedly declined to, so
a true overlay is not possible on GNOME as a plain Wayland client. The plan is a
second frontend on GTK4/libadwaita using a normal `xdg-toplevel`, sharing
`jump-core` unchanged.

Blur will carry over: `ext-background-effect-v1` is a merged wayland-protocols
staging protocol, already implemented by COSMIC, KWin 6.7+, and Niri, with GNOME
support landing in 51.

## Components

Three binaries share one library and one config store:

| Binary | What it is |
|---|---|
| `jump` | The launcher daemon and overlay. Running it again toggles the overlay. |
| `jump-applet` | A COSMIC panel applet — a button that activates the launcher over D-Bus. |
| `jump-settings` | The settings window. |

`jump-settings` and `jump` never talk to each other. The window writes
`cosmic-config`; the launcher subscribes to the same store and applies changes
live. That is the whole integration, and it is why no control has an apply
button — verified by clicking a toggle and watching the running daemon pick it
up without a restart.

The applet is deliberately not a popup. Everything the launcher does already
happens in a full-screen overlay with its own keyboard focus, so reproducing any
of it inside a panel popup would be a worse copy of the same UI. Its job is to
be a click target for people who would rather not memorise a shortcut. Add it in
**Settings → Desktop → Panel → Configure panel applets**.

## Settings and the control center

COSMIC Settings has no mechanism for third-party pages — its pages are compiled
in, and GNOME's control center is the same. So a launcher cannot put a page
there, and `jump` does the next best thing: it stores settings in
**cosmic-config**, the store COSMIC itself uses, and exposes the common actions
from the status-area icon.

The tray is applet-hosted rather than compositor-native: `cosmic-comp` implements
no tray protocol, and `cosmic-applet-status-area` owns
`org.kde.StatusNotifierWatcher` on the session bus. `jump` therefore registers a
standard `StatusNotifierItem`, which is also what makes the same icon work on KDE
and on GNOME with the AppIndicator extension. Removing the Status Area applet
removes the icon, so a missing host is handled as normal rather than as an error.

## Known rough edges

- First-run content indexing has not been timed to completion. Priority ordering
  means the useful documents land first, but the full pass is slow.
- In a light theme the accent-tinted selection reads pink; it likely wants a
  different treatment than the dark-theme tint.
- Desktop *actions* (e.g. VS Code's recent workspaces) render with the action
  description as the title, which is how pop-launcher returns them.
- The entrance animation is opacity plus a short rise. There is no scale
  component: iced's `Float` only applies a transform when scaling above 1.0, so
  a 0.96 → 1.0 entrance would silently render unscaled.
