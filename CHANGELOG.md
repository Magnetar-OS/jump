# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
