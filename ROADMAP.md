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
| Snippets / text expansion | ✅ | ⬜ | Buildable: cosmic-comp advertises `zwp_virtual_keyboard_manager_v1` (measured on a live 1.5 session). Injection needs a custom keymap, wtype-style — real work, but no protocol blocker |
| Quicklinks (URL templates) | ✅ | ✅ | `quicklinks` setting: keyworded URL templates, claiming the query; simpler than bundling plugins and live-reloads with the config store |
| Window switching | ✅ | ✅ | `ext-foreign-toplevel-list`, merged into search, Ctrl+W closes |
| Window management commands | ✅ | ⬜ | Protocol confirmed: `zcosmic_toplevel_manager_v1` v4 and `ext_workspace_manager_v1` are advertised (measured); maximize / minimize / move-to-workspace are implementable |
| System commands | ✅ | ✅ | Dark/light toggle, Settings deep links; session commands via pop-launcher |
| Media control | ✅ | ✅ | MPRIS play/pause/next/prev, current track in the subtitle |
| Power profiles | — | ✅ | power-profiles-daemon |
| Emoji & symbol picker | ✅ | 🔶 | `emoji …` keyword over compiled-in data; list view today, grid view pending (pillar 3) |
| Process search + kill | ✅ | ✅ | `kill …` claims the query; SIGTERM on Enter, Force Kill in the action panel |
| Bluetooth / Wi-Fi control | ✅ | ⬜ | bluez connect/disconnect; NetworkManager network switching |
| Action panel (⌘K) | ✅ | ✅ | Ctrl+K, keyboard-first. ⬜ v2: plugin `mods`, per-action shortcuts shown |
| Extensions / plugin API | ✅ | ✅ | Script-filter contract + pop-launcher plugins. ⬜ devex tooling below |
| Extension store | ✅ | 🚫→🔶 | No store service. System-wide plugin dir (`/usr/share/jump/plugins`) ✅; curated plugin list in the repo ⬜ |
| Per-command aliases & hotkeys | ✅ | ✅ | CLI deep links (`jump show clip` bound to a COSMIC custom shortcut) + `plugin_keywords` overrides; settings-window editor pending |
| Favorites / pinned results | ✅ | ✅ | Pin on Top in the action panel; pinned matches rank above everything |
| Fallback searches | ✅ | ✅ | `fallbacks` setting; rows appended below every search's results |
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
- [x] Plugin-supplied variables/state between query and activation
      (Alfred's `variables`): top-level and per-item, the item's winning,
      exported into the activation command's environment.
- [x] Streaming results (Alfred's `rerun`): a response naming `rerun` seconds
      (clamped 0.5–5.0) has its query re-run on that interval while the user
      is still looking at it; each answer schedules at most one rerun.
- [x] `jump plugin new <name>` scaffolds a runnable keyworded plugin into
      the user's plugin directory; `jump plugin lint <dir-or-name> [query]`
      checks the manifest, the commands it names, and one sample query's
      output against the item schema — by actually running it, because the
      violations that break a plugin live in its output.
- [x] Discovery of system-wide plugins: every `<data dir>/jump/plugins` on
      `$XDG_DATA_DIRS`, so distros can package them; the user's directory is
      searched first and shadows a packaged plugin of the same name.
- [x] Quicklinks: user-defined URL templates with `{query}` — landed as the
      `quicklinks` config key rather than bundled plugins, because a config
      list live-reloads, needs no scripts on disk, and the settings window
      can grow an editor for it (pillar 4).
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
- [x] "What's playing" inline in the media commands' subtitles: fetched off
      the frame when the overlay opens, re-running a visible search when the
      answer arrives — the match path itself stays synchronous.
- [ ] Window management commands over `cosmic-toplevel-management`:
      maximize, minimize, move to workspace. The protocols are there —
      `zcosmic_toplevel_manager_v1` v4 and `ext_workspace_manager_v1`,
      measured on a live 1.5 session — what remains is verifying which
      requests cosmic-comp honours from a third-party client.
- [ ] Bluetooth device connect/disconnect via bluez
- [ ] Wi-Fi network switching via NetworkManager
- [x] Process search + kill: `kill …` lists the user's own processes by
      name/cmdline/pid, heaviest first; Enter sends SIGTERM, the action panel
      offers Force Kill. The claim also stops the query fanning out to the
      other providers — the file search's own `plocate` child used to match
      the kill query it was spawned by and eat the signal.
- [x] Emoji search: `emoji …` claims the query, matches CLDR names and
      shortcodes from compiled-in data (no daemon, no I/O), Enter copies.
      List view today; the grid presentation is pillar 3.
- [ ] Clipboard images: capture non-text offers over `wlr-data-control`,
      thumbnail in a grid, copy back on Enter
- [x] Fallback searches: configurable "Search the web for …" rows appended
      below every ordinary search's results (`fallbacks`, DuckDuckGo and
      Wikipedia by default)
- [ ] Snippet expansion — **investigation resolved**: cosmic-comp advertises
      `zwp_virtual_keyboard_manager_v1` v1 and `zwp_input_method_manager_v2`
      v1 (measured on a live 1.5 session with a registry probe), so injection
      is possible. The remaining work is real but unblocked: a generated
      keymap carrying the snippet's symbols, wtype-style. The same mechanism
      unlocks paste-into-frontmost for clipboard entries (pillar 3).

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
- [x] Keyword overrides: `plugin_keywords` maps a plugin id to a replacement
      keyword without editing its manifest — the aliases feature. An empty
      keyword removes it. ⬜ the settings-window editor on top of it.
- [x] Favorites: Pin on Top in every result's action panel; a pinned result
      that matches the query at all ranks above everything unpinned
      (`favorites` config key). ⬜ manage the list in the settings window.
- [x] CLI deep links: `jump show <query>` opens with the query pre-filled,
      forwarded to the running daemon over D-Bus — a COSMIC custom shortcut
      bound to `jump show clip` is a per-command hotkey without jump owning
      any global keybinding state
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
- [x] `rust-toolchain.toml` agreeing with `rust-version` (1.98.0), plus
      `rustfmt.toml` (`imports_granularity = "Module"`)
- [ ] Sweep for hardcoded user-visible strings; everything through `fl!` so
      translation PRs are additive. Plurals through Fluent, never `format!`
- [x] Metainfo validated in CI: `desktop-file-validate` + `appstreamcli
      validate --no-net` run in the metadata job (and locally via
      `just validate`). ⬜ remaining completeness: `branding` colors and a
      `vcs-browser` URL.
- [x] Per-size icons under `hicolor/<size>/apps/` (16–256 ship already).
      ⬜ a `-symbolic` variant for the applet and tray.
- [ ] xdgen decision: adopt (with the `CARGO_TARGET_DIR` fix) or document the
      rejection — Peek found it broke single-instance via feature
      re-resolution, so verify before adopting
- [x] `hooks/pre-commit.hook` running `cargo fmt --check`, as the shipping
      apps do (`git config core.hooksPath hooks`)
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
- [x] Initial commit and repository hygiene: history starts at the initial
      import, conventional commits from there
- [x] CI: fmt + clippy pedantic + tests + metadata validation, toolchain
      taken from `rust-toolchain.toml`, plus a vendored offline resolution
      job so `just vendor` never rots (`.github/workflows/ci.yml`)
- [x] Test coverage where the logic is: `rank.rs` cross-source merging,
      `frecency.rs` decay, `plugin.rs` protocol conformance and discovery,
      file-ranking phase one, clipboard capping/permissions, emoji and web
      link matching — 79 tests across engine and frontend
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
