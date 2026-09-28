# Changelog — jump-core

All notable changes to the `jump-core` crate are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate
follows [Semantic Versioning](https://semver.org/). The `jump` application
keeps its own changelog at the repository root.

## [2.0.0] - Unreleased

Prepared, not yet published to crates.io.

### Changed (breaking)

- `plugin::Manifest::timeout` takes the keyword the plugin currently answers
  to. Without one, `timeout_ms` can only lower the 180 ms interactive
  default; only a keyworded plugin may take up to 3 s.
- `plugin::scaffold(root, name)` takes the plugin root rather than the
  finished directory, derives the directory from the name, and returns the
  directory and keyword. A name that cannot name a directory inside the root
  is refused with `InvalidInput`.

### Added

- `plugin::PluginHost::roots`, the discovery roots in shadowing order.
- `plugin::PluginHost::replace_plugins`, which takes a fresh discovery while
  keeping the disabled set and keyword overrides.
- `plugin::is_keyword` and `plugin::scaffold_keyword`.
- `PartialEq` and `Eq` for `plugin::Manifest` and `plugin::Plugin`.

### Fixed

- A plugin query that overruns its deadline or the 1 MiB output cap, or is
  abandoned by dropping its future, kills the plugin's whole process group.
  The output cap is enforced while reading, not after buffering everything.
- `plugin::lint` runs the query through the same limits as the host.
- `plugin::scaffold` escapes the name in the manifest and the sample script.

## [1.0.0] - 2026-09-10

Earlier releases (1.0.0, 0.1.1, 0.1.0) predate this file; see the git
history.
