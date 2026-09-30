# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Clear content index.** Switching "Search inside files" off keeps the
  index on disk so switching it back on is instant, which left no way to be
  rid of what it had read short of deleting the file by hand. Jump Settings
  now shows what the index occupies on disk with a Clear button beside it,
  and the status-area menu has a "Clear content index" item. The launcher
  stops the indexer and closes the index before deleting it; while content
  search is on, it starts a new index afterwards.

### Fixed

- `jump plugin import` of a workflow with several script filters is all or
  nothing even when moving a plugin into place fails part-way: the ones
  already moved are taken back out, where they used to be left behind beside
  the error.

## [1.1.2] - 2026-09-29

### Changed

- Rebuilt against the current COSMIC libraries (libcosmic `03d7dcb`).
- Jump Settings is categorised as desktop settings rather than the bare
  `Settings` category.

### Fixed

- A plugin that overruns its deadline or output cap is killed together with
  its process group, not just the script itself; background jobs no longer
  outlive the query. The 1 MiB output cap is enforced while reading, so
  a runaway plugin is cut off at the cap instead of being buffered in full
  until its deadline. `jump plugin lint` applies the same cap.
- Results no longer duplicate or reshuffle as providers answer. Each arrival
  (applications, files, contents, plugins) used to be merged into the
  already-ranked list, which repeated window, command and web-search rows and
  pushed exact filename matches below weaker content matches. A streaming
  plugin's rerun replaces its rows instead of adding a second copy, and
  content search stays out of a query a keyworded plugin has claimed.
- Changing a setting in Jump Settings no longer undoes a result pinned in the
  launcher, or a key edited by hand, since the window opened: it writes only
  the setting that changed, and follows changes made elsewhere.
- `jump plugin import` writes nothing unless the import succeeds: a filter
  that cannot be converted leaves no half-written directory, a name collision
  is detected before anything is written, and two filters sharing a keyword
  both import. A workflow can no longer make the importer read a script or
  icon from outside the bundle (`../`, absolute paths, symlinks, devices).
- A plugin installed, removed or edited while Jump runs takes effect without
  restarting it, as `jump plugin new` and the plugin guide always promised:
  the launcher and Jump Settings watch the plugin directories, and an open
  query is re-run when the plugin set changes.
- File and content search settings apply live, as Settings says they do.
  Switching "Search inside files" off stops the content indexer at its next
  chunk and stops querying the index at once; changing the indexed folders
  rebuilds the file index; and the rows either search had already put on
  screen leave it. Rebuilding the file index from the tray menu no longer
  starts a second content indexer alongside the first.
- `jump plugin import` no longer unpacks a whole `.alfredworkflow` into
  `/tmp`. It reads only `info.plist` and the scripts and icons that name,
  each capped at 8 MiB and 32 MiB per import while decompressing, so a zip
  bomb is refused instead of filling memory-backed `/tmp`.
- Typing no longer leaves every superseded query's plugins, file search and
  content search running to completion or their deadline: each keystroke
  cancels the work the previous one started, and a cancelled plugin is
  killed with its process group.
- A plugin without a keyword — which runs on every keystroke — is held to the
  180 ms interactive deadline even if its manifest sets `timeout_ms`; only a
  keyworded plugin may take up to 3 s, as documented. The deadline follows
  the keyword the plugin currently answers to, so removing its keyword in
  Settings removes the allowance. `jump plugin import` no longer writes
  `timeout_ms = 3000` for keywordless filters, and `jump plugin lint` says
  why a keywordless `timeout_ms` is clamped.
- Activating a result that nothing can act on any more — a plugin switched
  off since the query, the launcher service down, no clipboard backend — no
  longer counts as a use for ranking, and is logged.
- `jump plugin new` accepts any name: quotes, `%` and `'` no longer produce
  an unparseable manifest or a broken script, the sample script's JSON
  output is escaped, and a name such as `../x` that cannot name a directory
  inside the plugin folder is refused.
- A plugin keyword is held to one rule everywhere: Jump Settings no longer
  saves a keyword override containing a space, which could never match, and
  `jump plugin import` imports such an Alfred keyword without a keyword and
  says so, as `jump plugin lint` already reported for manifests.

## [1.1.1] - 2026-09-22

### Changed

- Rebuilt against the current COSMIC libraries (libcosmic `03c8f93`). 1.1.0
  was tagged but never released: its changelog section was missing, and the
  release pipeline refused it.

## [1.0.2] - 2026-09-21

### Bug Fixes

- **packaging:** start the installed jump, not ~/.local/bin/jump ([bdfcd62](https://github.com/Magnetar-OS/jump/commit/bdfcd62))

## [1.0.1] - 2026-09-16

### Fixed

- The Arch package is valid. The v1.0.0 pacman package carried a tar entry
  with an empty name, so the repository refused it and Jump never reached
  `pacman -S`. The package is now built one directory tree at a time.

## [0.1.0] - 2026-09-08

### Fixed

- The deb, rpm, Arch and AUR packages declared themselves MPL-2.0. That is
  the licence of the `jump-core` engine crate; the binaries in those packages
  are GPL-3.0-only, as the AppStream metainfo already stated.
- The selection highlight was noticeably weaker in a light theme than a dark
  one; its opacity now follows the theme so both carry the same weight.

- The action panel showed a blank clickable row under every application's
  desktop actions, and labelled those actions with their internal group id
  (`new-private-window`) instead of their name ("New Incognito Window").
- Clipboard subtitles said "1 characters"; counts are pluralised by the
  Fluent catalogue now, which also makes them translatable.
- Window subtitles and clipboard subtitles were hardcoded English; both go
  through the catalogue.

### Added

- `jump-core`, the UI-free engine, is published to crates.io so a second
  frontend can depend on it as a normal crate rather than a path.
- A first-run hint naming Ctrl+K and the keyword providers, shown until the
  launcher has been used once.
- The action panel now explains why the selected result ranks where it does:
  score, activation count, how long since the last one, and the recency
  weight that age earns.

- Calculator and unit conversion behind `=`, answered by Qalculate directly:
  `= 15*3` gives `45`, `= 5 km to miles` converts. The answer is shown in
  large type and Enter copies it. pop-launcher's own calc plugin is not used
  because it answers `<expr> x = ?` for every input against Qalculate 5.12.

- `jump plugin import` converts Alfred workflows (`.alfredworkflow` bundles
  or extracted directories) into jump plugins — script filters map one to
  one, with unconvertible pieces reported as warnings.
- A Search providers section in Settings: open windows, system commands,
  Bluetooth/Wi-Fi, clipboard, emoji and web results each toggle off.
- A "Reduce motion" setting that collapses the entrance, row cascade and
  page slide to plain fades, applied live.

- Emoji search: `emoji party` lists matching emoji by name and shortcode;
  Enter copies the emoji to the clipboard.
- Quicklinks: user-defined URL templates with a claiming keyword
  (`quicklinks` setting) — `yt cats` opens a YouTube search.
- Fallback web searches appended below every search's results
  (`fallbacks` setting, DuckDuckGo and Wikipedia by default), so a query
  that matched nothing still ends somewhere useful.
- Media commands now show the current track ("Artist — Title") as their
  subtitle while something is playing.
- Plugins are also discovered system-wide under `<data dir>/jump/plugins`
  on `$XDG_DATA_DIRS` (e.g. `/usr/share/jump/plugins`), so distributions
  can package them; a user plugin with the same directory name wins.
- `jump show <query>` opens the launcher with the query pre-filled — bind
  it to a COSMIC custom shortcut for a per-command hotkey, e.g.
  `jump show clip` straight into clipboard history.
- The action panel offers "Copy Link" on web results.
- Settings window: a keyword field per plugin (the manifest's keyword is the
  placeholder, so clearing restores it) and a Pinned results section for
  managing favorites.
- Performance budgets for the interactive path, run as tests and printed by
  `just bench`.
- A reviewed plugin directory in `docs/plugins.md`, with the trust model
  stated plainly.
- Plugin `mods`: alternate actions on modifier+Enter (Ctrl/Alt/Shift/Super),
  also listed in the action panel. Each action row now shows its key hint.
- Bluetooth and Wi-Fi control: paired Bluetooth devices and saved Wi-Fi
  networks appear as connect/disconnect results, found by their own name
  or by typing "bluetooth" or "wifi".
- Window management from the action panel (Ctrl+K on a window result):
  Maximize/Restore, Minimize, and Full Screen/Exit Full Screen, with the
  entries following the window's current state.
- Keyword aliases: the `plugin_keywords` setting overrides any plugin's
  keyword without editing its manifest (empty removes the keyword).
- Favorites: every result's action panel (Ctrl+K) offers "Pin on Top";
  a pinned result that matches the query ranks above everything unpinned.
- `jump plugin new <name>` scaffolds a runnable plugin; `jump plugin lint`
  checks a plugin's manifest, commands, and sample output for problems.
- Plugins can carry Alfred-style `variables` from query to activation
  (exported as environment variables to the action command) and stream
  updates with Alfred's `rerun` (re-query on an interval, clamped 0.5–5 s,
  while the query is on screen).

[Unreleased]: https://github.com/Magnetar-OS/jump/compare/v1.1.2...HEAD
[1.1.2]: https://github.com/Magnetar-OS/jump/compare/v1.1.1...v1.1.2
[1.1.1]: https://github.com/Magnetar-OS/jump/compare/v1.0.2...v1.1.1
[1.0.2]: https://github.com/Magnetar-OS/jump/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/Magnetar-OS/jump/compare/v0.1.0...v1.0.1
[0.1.0]: https://github.com/Magnetar-OS/jump/releases/tag/v0.1.0
