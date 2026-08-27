# Roadmap: a Raycast-class launcher for Linux

The goal, stated once: jump should be to COSMIC (and later GNOME) what Raycast
is to macOS — a launcher that is also the fastest way to *do* things, extended
by plugins anyone can write, integrated from the desktop's theme store down to
logind, with a UI polished enough that the polish itself is a feature.

That decomposes into four directions, and each has a pillar or a matrix below:

1. **Feature parity** — every capability Raycast ships either has a jump
   equivalent, a plugin path, or a documented reason it cannot exist on
   Wayland. The matrix is the contract; 1.0 means no row is left undecided.
2. **Pixel-perfect, delightful UI** — geometry is documented and deliberate,
   every state change transitions rather than snaps, and interaction depth
   (action panel, inline detail, grid views) matches the reference apps.
3. **Total COSMIC integration** — the conventions in
   [`cosmic-conventions.md`](cosmic-conventions.md) are the checklist, not a
   suggestion. Config in cosmic-config, theme tokens only, frosted keys
   gating blur, correct launch sequence, applet, tray, i18n, metainfo.
4. **Engineering quality** — Wayland-native (layer-shell, no X fallback),
   wgpu-rendered, measured performance budgets, CI that validates what the
   compiler cannot, and a core/frontend split that keeps the engine reusable.

Each pillar is shippable on its own and no pillar depends on a later one.
Checked items are landed and verified on a live COSMIC 1.5 session.

## Feature parity matrix

The reference is Raycast (with Alfred's workflow model where it is the better
fit — jump's plugin format is already Alfred's script-filter contract).
**Status**: ✅ shipped · 🔶 partial · ⬜ planned · 🚫 non-goal with reason.

| Capability | Raycast | jump | Plan |
|---|---|---|---|
| App search + launch | ✅ | ✅ | Frecency-ranked, activation token, GPU env, systemd scope |
| Calculator / unit conversion | ✅ | ✅ | Via pop-launcher (qalc). ⬜ verify currency rates work offline-degraded; ⬜ large-type inline answer row |
| File search | ✅ | ✅ | Private plocate index, two-phase ranking |
| Full-text file search | 🔶 | ✅ | FTS5, opt-in, ahead of Raycast here |
| Clipboard history | ✅ | 🔶 | Text only today. ⬜ images (grid view), ⬜ paste-into-frontmost (needs virtual-keyboard or data-control paste path) |
| Snippets / text expansion | ✅ | ⬜ | Depends on `zwp_virtual_keyboard_v1` on cosmic-comp — investigate before promising |
| Quicklinks (URL templates) | ✅ | ⬜ | Fits the plugin format; ship as bundled plugins |
| Window switching | ✅ | ✅ | `ext-foreign-toplevel-list`, merged into search, Ctrl+W closes |
| Window management commands | ✅ | ⬜ | maximize / minimize / move-to-workspace via `cosmic-toplevel-management`; tiling toggle via cosmic-comp config — investigate protocol coverage first |
| System commands | ✅ | ✅ | Dark/light toggle, Settings deep links; session commands via pop-launcher |
| Media control | ✅ | ✅ | MPRIS play/pause/next/prev. ⬜ "what's playing" in subtitles |
| Power profiles | — | ✅ | power-profiles-daemon |
| Emoji & symbol picker | ✅ | ⬜ | Data file, no daemon; grid view |
| Process search + kill | ✅ | ✅ | `kill …` claims the query; SIGTERM on Enter, Force Kill in the action panel |
| Bluetooth / Wi-Fi control | ✅ | ⬜ | bluez connect/disconnect; NetworkManager network switching |
| Action panel (⌘K) | ✅ | ✅ | Ctrl+K, keyboard-first. ⬜ v2: plugin `mods`, per-action shortcuts shown |
| Extensions / plugin API | ✅ | ✅ | Script-filter contract + pop-launcher plugins. ⬜ devex tooling below |
| Extension store | ✅ | 🚫→⬜ | No store service. ⬜ distro-packageable system-wide plugin dir + a curated plugin list in the repo |
| Per-command aliases & hotkeys | ✅ | ⬜ | Keyword editor (aliases); CLI deep links (`jump show clip`) so COSMIC custom shortcuts become per-command hotkeys |
| Favorites / pinned results | ✅ | ⬜ | Pin above frecency |
| Fallback searches | ✅ | ⬜ | "Search web for …" rows when nothing matches |
| Menu-bar search of frontmost app | ✅ | 🚫 | No Wayland protocol exposes another client's menus; not buildable honestly |
| Cloud sync / AI | ✅ | 🚫 | No network services in core. Config is plain files in cosmic-config — dotfile-syncable by design. AI is a plugin if anyone wants it |

## Pillar 1 — Plugin platform

The Alfred script-filter host in `jump-core` is the foundation: a manifest plus
an executable that gets the query on argv and prints JSON. pop-launcher plugins
keep working through the service alongside it.

- [x] Manifest + JSON protocol, keyword claiming, concurrent queries, hard
      deadlines, output caps (`crates/jump-core/src/plugin.rs`)
- [x] `uid` — stable item identity. Keys the row, which feeds the launcher's
      own frecency: plugin items now *learn* like applications do.
- [x] `autocomplete` — Tab replaces the query, Alfred-style.
- [x] `timeout_ms` manifest override (clamped 50–3000 ms) so a keyworded
      network plugin can be slower than the 180 ms interactive default without
      every plugin being allowed to.
- [x] Per-plugin enable/disable: `disabled_plugins` in the config store, a
      Plugins section in the settings window, applied live via `watch_config`.
- [ ] `mods` — alternate actions on modifier+Enter (surfaces in the action
      panel, pillar 3).
- [ ] Plugin-supplied variables/state between query and activation
      (Alfred's `variables`).
- [ ] Streaming results (rerun-on-interval, Alfred's `rerun`).
- [ ] A `jump plugin new <name>` scaffolder and a `jump plugin lint` that
      checks a manifest + sample output against the schema.
- [ ] Discovery of system-wide plugins (`/usr/share/jump/plugins`) so distros
      can package them.
- [ ] Bundled quicklinks plugin: user-defined URL templates with `{query}`.
- [ ] Curated plugin list in the repository (the store, without a service).
- [ ] Import: Alfred workflow converter — script filters map nearly 1:1.

## Pillar 2 — Deep integration

High-level to low-level, in the order the wiring already exists. The rule that
holds this pillar together: never duplicate what pop-launcher already ships
(lock/logout/suspend/restart/shutdown/BIOS are its session scripts and already
flow through the bridge).

- [x] Applications, windows (`ext-foreign-toplevel`), files (private plocate
      index), full-text (FTS5), clipboard (`wlr-data-control`), calculator /
      web / terminal via pop-launcher
- [x] Launch correctness: XDG activation token, switcheroo-control GPU
      environment, own systemd scope
- [x] System commands provider (`src/system.rs`): dark/light mode toggle
      (writes the same `com.system76.CosmicTheme.Mode` key COSMIC Settings
      does), deep links into twelve COSMIC Settings pages, matched on a
      vocabulary wider than their titles ("wifi" → Network)
- [x] MPRIS media control: Play/Pause, Next Track, Previous Track. Addresses
      the player that is actually playing, else the first — what hardware
      media keys do — and skips `playerctld`, which would double every real
      player. Degrades to a log line when nothing is running.
- [x] power-profiles-daemon: Performance / Balanced / Power Saver commands,
      writing the same `ActiveProfile` property `powerprofilesctl set` does.
      Offered only when the daemon's tooling is installed.
- [ ] "What's playing" inline in the media commands' subtitles (needs async
      results on the match path)
- [ ] Window management commands over `cosmic-toplevel-management`:
      maximize, minimize, move to workspace — investigate which requests
      cosmic-comp actually honours from a third-party client before listing
      individual commands.
- [ ] Bluetooth device connect/disconnect via bluez
- [ ] Wi-Fi network switching via NetworkManager
- [x] Process search + kill: `kill …` lists the user's own processes by
      name/cmdline/pid, heaviest first; Enter sends SIGTERM, the action panel
      offers Force Kill. The claim also stops the query fanning out to the
      other providers — the file search's own `plocate` child used to match
      the kill query it was spawned by and eat the signal.
- [ ] Emoji and unicode search (a data file, no daemon)
- [ ] Clipboard images: capture non-text offers over `wlr-data-control`,
      thumbnail in a grid, copy back on Enter
- [ ] Fallback searches: configurable "Search the web for …" rows when a
      query matches nothing
- [ ] Snippet expansion — needs a virtual keyboard protocol; investigate
      `zwp_virtual_keyboard_v1` on cosmic-comp before promising it

## Pillar 3 — UI: pixel-perfect and delightful

The overlay already has the identity pieces: entrance animation with
interruptible reverse, per-row cascade, compositor blur shaped to the panel,
optical centring, selection pills. What Raycast has that jump does not is a
second dimension of interaction on each row.

- [x] Pixel-level geometry documented and deliberate (`src/surface.rs`)
- [x] Theme-correct in light and dark, `AppType::System` frosting gated on
      the desktop's own `frosted_system_interface` key
- [x] Icon handles resolved off the draw path, explicit fallback chains,
      one-line ellipsizing
- [x] **Action panel v1** (the defining Raycast interaction): Ctrl+K on a
      result opens a floating card of everything else it can do — file: open,
      reveal, copy path, trash; window: focus, close; clipboard entry: copy;
      application: pop-launcher's context options ("New Window" and friends)
      appended as they arrive. Keyboard-first: Ctrl+K, arrows, Enter, Escape
      peels one layer.
- [ ] Action panel v2: plugin `mods`, per-action shortcuts shown in the card,
      paste-into-frontmost for clipboard entries.
- [ ] Inline detail for calculator/conversion results (large-type answer row)
- [ ] Grid view for visual results (emoji, clipboard images) reusing the
      Launchpad grid machinery
- [ ] Per-row secondary text alignment pass at 1×/1.25×/2× scale factors —
      pixel-perfect means at every scale factor, verified by screenshot
      comparison, not by eye once
- [ ] Light-theme selection treatment: the accent tint that works on dark
      reads pink on light; design it separately rather than sharing the value
- [ ] Desktop actions render with the action description as the title (as
      pop-launcher returns them); re-title as "App — Action"
- [ ] Reduced-motion: honour the system animation preference by collapsing
      transitions to fades, not by branching every animation site
- [ ] Screen-reader pass over the result list (libcosmic `a11y` is already
      enabled; verify rows announce title + subtitle + position)
- [ ] Entrance scale component when upstream allows it: iced's `Float` only
      transforms above 1.0, so 0.96 → 1.0 renders unscaled today — upstream
      issue or local widget, decide once

## Pillar 4 — Settings depth

- [x] Live-reload settings window (blur, opacities, grid layout/paging/size,
      file search corpus, content indexing, plugins on/off) — all applied
      without restart
- [x] Config in the cosmic-config store, one key per field, versioned
- [ ] Search-provider section: toggle and re-order providers (windows, files,
      content, clipboard, system, each pop-launcher plugin)
- [ ] Per-provider result caps and the rank weights exposed as "advanced"
- [ ] Keyword editor: override any plugin's keyword without editing its
      manifest — this is also the aliases feature
- [ ] Favorites: pin a result above frecency, manage the list in settings
- [ ] CLI deep links: `jump show <query>` maps a COSMIC custom shortcut to a
      pre-filled query ("open straight into clipboard history") — per-command
      hotkeys without owning global keybinding state
- [ ] Frecency inspector ("why is this ranked here?") and a reset button
- [ ] First-run experience: a short, dismissable hint row (bind a shortcut,
      try Ctrl+K) instead of an empty launchpad with no explanation

## Pillar 5 — COSMIC conventions compliance

[`cosmic-conventions.md`](cosmic-conventions.md) is the audit standard — what
eight COSMIC codebases agree on, measured on a live session. jump already
follows the load-bearing ones (RDNN identity everywhere, cosmic-config with
live watch, theme tokens, frosted gating, layer-surface create/destroy, the
three-step launch sequence, justfile with `rootdir`/`vendor`). What remains is
the long tail that separates "works" from "indistinguishable from first-party":

- [x] One RDNN id (`dev.entro314labs.Jump`) across config store, desktop
      entries, metainfo, icons, D-Bus
- [x] i18n-embed + Fluent + `fl!` wired (`src/localize.rs`), catalogue layout
      Weblate expects
- [x] justfile with `rootdir` / `prefix` / `cargo-target-dir`, vendoring for
      offline packaging builds
- [ ] `rust-toolchain.toml` agreeing with `rust-version`, plus `rustfmt.toml`
      (`imports_granularity = "Module"`) — adopt before the tree grows
- [ ] Sweep for hardcoded user-visible strings; everything through `fl!` so
      translation PRs are additive. Plurals through Fluent, never `format!`
- [ ] Metainfo completeness: `com.system76.CosmicApplication` provides,
      `requires`/`supports`, `branding` colors, release entries matching
      Cargo.toml, reachable URLs; `desktop-file-validate` +
      `appstreamcli validate` in CI (`--no-net` on PRs)
- [ ] Per-size icons under `hicolor/<size>/apps/` (small sizes drawn on the
      pixel grid), symbolic icon for the applet and tray
- [ ] xdgen decision: adopt (with the `CARGO_TARGET_DIR` fix) or document the
      rejection — Peek found it broke single-instance via feature
      re-resolution, so verify before adopting
- [ ] `hooks/pre-commit.hook` running `cargo fmt --check`, as the shipping
      apps do
- [ ] Track libcosmic's default branch deliberately: a recurring
      `cargo update -p libcosmic` + build + smoke-test routine, because
      unpinned-git-dependency breakage should be found by us, not packagers

## Pillar 6 — Engineering quality and performance

The stack is already the modern one — Wayland-only via wlr-layer-shell, wgpu
rendering, tokio, zbus, edition 2024, fat-LTO release profile. This pillar
makes the quality claims *verifiable* instead of asserted.

- [x] Core/frontend split: `jump-core` (MPL-2.0, no UI deps) under the
      GPL-3.0-only COSMIC frontend, so a GNOME frontend can link it
- [x] Release profile tuned for cold start (fat LTO, one codegen unit,
      stripped); dev profile keeps dependencies optimised for dogfooding
- [ ] Initial commit and repository hygiene: the tree is `git init`ed but has
      no history yet — land the initial commit, then conventional commits
      from there
- [ ] CI: build + `cargo clippy --all-features --locked -- -W clippy::pedantic`
      + `cargo test` + metadata validation, toolchain taken from
      `rust-toolchain.toml`. A vendored offline build in CI too, so
      `just vendor` never rots
- [ ] Test coverage where the logic is: `rank.rs` cross-source merging,
      `frecency.rs` decay, `plugin.rs` protocol conformance (manifest
      parsing, deadline kill, output caps), file-ranking phase one (pure
      string scoring — trivially testable), clipboard capping/permissions
- [ ] Performance budgets, measured in CI or a `just bench` recipe, not
      remembered: keybind → first frame (the number the launcher is judged
      on), keystroke → results painted (pop-launcher answers in 0.5–1.5 ms;
      the budget is ours to spend), memory resident as a daemon, index build
      time on the reference corpus
- [ ] Time first-run content indexing to completion on a real home directory
      and publish the number (README currently says it has not been done)
- [ ] Packaging: `packaging/linux` exists — finish deb recipe, AUR/COPR, and
      a Flatpak decision (layer-shell + wlr-data-control through the sandbox
      is the open question; document the answer either way)
- [ ] Release automation: `release.config.json` is present — wire tag →
      changelog → GitHub release with the vendored tarball attached
- [ ] CHANGELOG discipline: every user-facing change lands with its entry

## Sequencing

No dates — order only. Each milestone is releasable.

- **v0.2 — foundation**: initial commit, CI, toolchain files, tests for
  `jump-core`, metadata validation, first packaged build. (Pillars 5–6 core)
- **v0.3 — provider depth**: emoji, process kill, quicklinks, fallback
  searches, "what's playing", clipboard images. (Pillar 2)
- **v0.4 — interaction depth**: action panel v2, `mods`, inline detail,
  grid view, scale-factor and light-theme passes. (Pillars 1, 3)
- **v0.5 — control**: provider settings, keyword editor/aliases, favorites,
  CLI deep links, frecency inspector, first-run hints. (Pillar 4)
- **v0.6 — reach**: Bluetooth, Wi-Fi, window management commands, snippet
  investigation resolved, plugin scaffolder/linter, Alfred importer,
  system-wide plugin dir. (Pillars 1, 2)
- **v1.0 — parity**: every matrix row is ✅ or 🚫-with-reason, every pillar-5
  item checked, the performance budgets hold in CI, and packaging covers at
  least deb + one rolling distro.
- **Post-1.0 — GNOME frontend**: GTK4/libadwaita on an `xdg-toplevel`,
  sharing `jump-core` unchanged; blur via `ext-background-effect-v1` (GNOME
  51+).

## Non-goals, so they stay decided

- No Electron/web runtime for plugins. The script-filter contract is the API;
  a plugin is any executable.
- No network services in the core. Anything network-backed is a plugin with a
  raised `timeout_ms`. Sync is "your config is plain files"; AI is a plugin.
- No duplication of pop-launcher's session scripts.
- No X11 path. Wayland layer-shell is the platform; GNOME gets a real
  frontend, not a compatibility hack.
- No menu-bar search of the frontmost application: no Wayland protocol
  exposes another client's menus, and pretending otherwise means shipping
  something that silently does nothing.
- No GNOME frontend before the COSMIC one is feature-stable — but everything
  in `jump-core` stays UI-free (MPL-2.0) so that door stays open.
