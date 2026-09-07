# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

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
