// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Alfred workflow import.
//!
//! Alfred script filters and jump plugins speak the same language: a program
//! receives the query and prints `{"items": […]}`, and jump's item schema
//! already accepts Alfred's fields — `uid`, `arg`, `autocomplete`, `valid`,
//! `icon`, `variables`, `mods`, `rerun` — so a filter's *output* needs no
//! translation at all. What an `.alfredworkflow` bundle has and a plugin
//! lacks is packaging: an `info.plist` describing objects and the connections
//! between them. This module reads that packaging and writes ours.
//!
//! One plugin directory is created per script filter. The filter's script
//! becomes the query command (Alfred's `{query}` placeholder rewritten to
//! `"$1"`, which is where jump passes the text), and the object the filter
//! connects to becomes the activation command where the mapping is honest:
//! an Open URL action becomes `xdg-open`, a Run Script action runs as given,
//! a Copy to Clipboard output becomes `wl-copy`. Anything else is reported
//! as a warning rather than silently dropped — a converter that pretends it
//! understood everything produces plugins that half work.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use plist::Value;

/// One plugin the import produced.
#[derive(Debug)]
pub struct Imported {
    pub directory: PathBuf,
    pub name: String,
    pub keyword: Option<String>,
}

/// The outcome of an import: what was created, and what could not be carried
/// over. Warnings are for the user's eyes; an import with warnings still
/// produced runnable plugins.
#[derive(Debug, Default)]
pub struct Import {
    pub plugins: Vec<Imported>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read the workflow: {0}")]
    Io(#[from] std::io::Error),
    #[error("info.plist did not parse: {0}")]
    Plist(#[from] plist::Error),
    #[error("no info.plist found — is this an Alfred workflow?")]
    NotAWorkflow,
    #[error("the workflow contains no script filters; only script-filter workflows convert")]
    NoScriptFilters,
    #[error("extracting the bundle needs `bsdtar`, which is not installed")]
    NoExtractor,
    #[error("{0} already exists; remove it or import elsewhere")]
    AlreadyExists(PathBuf),
}

/// Import `workflow` — an `.alfredworkflow` bundle or an extracted directory —
/// creating one plugin per script filter under `destination_root`.
pub fn import(workflow: &Path, destination_root: &Path) -> Result<Import, Error> {
    // A bundle is a zip; extract it somewhere disposable first. `tempfile`
    // gives the directory a 0700 mode and an unpredictable name — /tmp is
    // shared, and a predictable path is a symlink target — and removes it
    // when the guard drops.
    let (source, _extracted) = if workflow.is_dir() {
        (workflow.to_path_buf(), None)
    } else {
        let extracted = extract(workflow)?;
        (extracted.path().to_path_buf(), Some(extracted))
    };

    let manifest_path = source.join("info.plist");
    if !manifest_path.is_file() {
        return Err(Error::NotAWorkflow);
    }
    let info = Value::from_file(&manifest_path)?;

    let workflow_name = string_key(&info, "name").unwrap_or_else(|| "Imported workflow".into());
    let workflow_description = string_key(&info, "description").unwrap_or_default();

    // Objects by uid, so connections can be followed.
    let objects: HashMap<String, &plist::Dictionary> = info
        .as_dictionary()
        .and_then(|dict| dict.get("objects"))
        .and_then(Value::as_array)
        .map(|objects| {
            objects
                .iter()
                .filter_map(|object| {
                    let object = object.as_dictionary()?;
                    let uid = object.get("uid")?.as_string()?.to_owned();
                    Some((uid, object))
                })
                .collect()
        })
        .unwrap_or_default();

    let connections = info
        .as_dictionary()
        .and_then(|dict| dict.get("connections"))
        .and_then(Value::as_dictionary);

    let mut import = Import::default();

    let filters: Vec<(&String, &&plist::Dictionary)> = objects
        .iter()
        .filter(|(_, object)| {
            object.get("type").and_then(Value::as_string)
                == Some("alfred.workflow.input.scriptfilter")
        })
        .collect();

    if filters.is_empty() {
        return Err(Error::NoScriptFilters);
    }

    // Deterministic order: plists have no meaningful object order once
    // indexed by uid, and a re-run should fail on the same directory.
    let mut filters = filters;
    filters.sort_by(|a, b| a.0.cmp(b.0));

    for (uid, filter) in filters {
        match convert_filter(
            uid,
            filter,
            &objects,
            connections,
            &source,
            destination_root,
            &workflow_name,
            &workflow_description,
            &mut import.warnings,
        ) {
            Ok(imported) => import.plugins.push(imported),
            // A directory collision aborts the whole import: partial output
            // plus an error is the worst of both worlds.
            Err(error @ Error::AlreadyExists(_)) => return Err(error),
            Err(error) => import
                .warnings
                .push(format!("script filter {uid} skipped: {error}")),
        }
    }

    if import.plugins.is_empty() {
        return Err(Error::NoScriptFilters);
    }
    Ok(import)
}

/// Convert one script filter and its primary connection.
#[allow(clippy::too_many_arguments)]
fn convert_filter(
    uid: &str,
    filter: &plist::Dictionary,
    objects: &HashMap<String, &plist::Dictionary>,
    connections: Option<&plist::Dictionary>,
    source: &Path,
    destination_root: &Path,
    workflow_name: &str,
    workflow_description: &str,
    warnings: &mut Vec<String>,
) -> Result<Imported, Error> {
    let config = filter
        .get("config")
        .and_then(Value::as_dictionary)
        .cloned()
        .unwrap_or_default();

    let keyword = config
        .get("keyword")
        .and_then(Value::as_string)
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(ToOwned::to_owned);

    // Alfred allows `{var:…}` templates in keywords; jump keywords are
    // literal. The plugin still imports — as an always-on plugin — but the
    // difference is worth a warning.
    let keyword = match keyword {
        Some(keyword) if keyword.contains('{') => {
            warnings.push(format!(
                "keyword {keyword:?} is templated; imported without a keyword — set one in \
                 manifest.toml"
            ));
            None
        }
        other => other,
    };

    let title = filter
        .get("title")
        .and_then(Value::as_string)
        .filter(|title| !title.is_empty())
        .unwrap_or(workflow_name)
        .to_owned();

    // The slug becomes a directory name and the keyword comes straight out of
    // an untrusted plist, so it is sanitised exactly like the title fallback —
    // a workflow declaring keyword `../../.config/...` must not write outside
    // the plugin root. The manifest keyword keeps the declared text; only the
    // path is constrained.
    let slug = slugify(keyword.as_deref().unwrap_or(&title));
    let directory = destination_root.join(&slug);
    // Belt and braces: slugify leaves no separators, but a landing spot
    // outside the root would be a write primitive, so it is checked, not
    // assumed.
    if !directory.starts_with(destination_root) {
        return Err(Error::Io(std::io::Error::other(
            "refusing to import outside the plugin directory",
        )));
    }
    if directory.exists() {
        return Err(Error::AlreadyExists(directory));
    }

    let query = script_from(&config, source)?;
    std::fs::create_dir_all(&directory)?;
    let query_file = write_script(&directory, "search", &query)?;

    // The object this filter feeds is the activation.
    let activate = connections
        .and_then(|connections| connections.get(uid))
        .and_then(Value::as_array)
        .and_then(|links| {
            // The unmodified connection is the Enter action. Modifier
            // connections in the workflow graph have no jump equivalent —
            // item-level `mods` in the JSON keep working regardless.
            let unmodified = links.iter().find(|link| {
                link.as_dictionary()
                    .and_then(|link| link.get("modifiers"))
                    .and_then(Value::as_signed_integer)
                    .unwrap_or(0)
                    == 0
            });
            if links.len() > 1 {
                warnings.push(
                    "modifier connections in the workflow graph were not converted; \
                     item-level `mods` in the filter's JSON still work"
                        .to_owned(),
                );
            }
            unmodified?
                .as_dictionary()?
                .get("destinationuid")?
                .as_string()
                .map(ToOwned::to_owned)
        })
        .and_then(|destination| objects.get(&destination))
        .and_then(|action| convert_action(action, source, &directory, warnings).transpose())
        .transpose()?;

    // Alfred workflows were written with no deadline at all; give them the
    // ceiling rather than the 180 ms interactive default.
    let mut manifest = format!(
        "name = \"{}\"\ndescription = \"{}\"\n",
        toml_escape(&title),
        toml_escape(workflow_description),
    );
    if let Some(keyword) = &keyword {
        let _ = writeln!(manifest, "keyword = \"{}\"", toml_escape(keyword));
    }
    let _ = writeln!(manifest, "query = \"./{query_file}\"");
    if let Some(activate) = &activate {
        let _ = writeln!(manifest, "activate = \"./{activate}\"");
    }
    if let Some(icon) = copy_icon(uid, source, &directory)? {
        let _ = writeln!(manifest, "icon = \"{}\"", toml_escape(&icon));
    }
    manifest.push_str("# Imported from Alfred, which has no query deadline; tune down once\n");
    manifest.push_str("# you know how fast it answers.\ntimeout_ms = 3000\n");
    std::fs::write(directory.join("manifest.toml"), manifest)?;

    Ok(Imported {
        directory,
        name: title,
        keyword,
    })
}

/// The filter's query program, as (shebang-prefixed) script text.
fn script_from(config: &plist::Dictionary, source: &Path) -> Result<String, Error> {
    // `scriptfile` wins when both are present, matching Alfred.
    if let Some(file) = config
        .get("scriptfile")
        .and_then(Value::as_string)
        .filter(|file| !file.is_empty())
    {
        let text = std::fs::read_to_string(source.join(file))?;
        return Ok(rewrite_query_placeholder(&text, argv_style(config)));
    }

    let script = config
        .get("script")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let language = config
        .get("type")
        .and_then(Value::as_signed_integer)
        .unwrap_or(0);

    let Some(interpreter) = interpreter_for(language) else {
        return Err(Error::Io(std::io::Error::other(format!(
            "script language {language} (AppleScript/JXA) does not exist on Linux"
        ))));
    };

    let body = rewrite_query_placeholder(script, argv_style(config));
    Ok(format!("#!/usr/bin/env {interpreter}\n{body}\n"))
}

/// Whether the filter receives the query as `$1` (argv) or as a `{query}`
/// placeholder Alfred substitutes into the script text.
fn argv_style(config: &plist::Dictionary) -> bool {
    config
        .get("scriptargtype")
        .and_then(Value::as_signed_integer)
        .unwrap_or(0)
        == 1
}

/// Rewrite Alfred's `{query}` placeholder to `"$1"`, where jump puts the text.
///
/// The quoted form is handled first so `"{query}"` does not end up as
/// `""$1""`.
fn rewrite_query_placeholder(script: &str, argv: bool) -> String {
    if argv {
        return script.to_owned();
    }
    script
        .replace("\"{query}\"", "\"$1\"")
        .replace("'{query}'", "\"$1\"")
        .replace("{query}", "\"$1\"")
}

/// Alfred's script-language codes. `AppleScript` and JXA are `None`: nothing
/// on a Linux box runs them, and a plugin that fails on every query is worse
/// than a refusal at import time.
const fn interpreter_for(language: i64) -> Option<&'static str> {
    match language {
        0 => Some("bash"),
        1 => Some("php"),
        2 => Some("ruby"),
        3 => Some("python3"),
        4 => Some("perl"),
        5 => Some("zsh"),
        _ => None,
    }
}

/// Fixed activation templates. The untrusted text lives in `open.data`;
/// `${template//'{query}'/$1}` is bash's literal pattern replacement, which
/// treats both the pattern and the substituted argument as data.
const OPEN_URL_TEMPLATE: &str = "#!/usr/bin/env bash
template=\"$(cat -- \"$(dirname -- \"$0\")/open.data\")\"
exec xdg-open \"${template//'{query}'/$1}\"
";

const COPY_TEMPLATE: &str = "#!/usr/bin/env bash
template=\"$(cat -- \"$(dirname -- \"$0\")/open.data\")\"
printf '%s' \"${template//'{query}'/$1}\" | wl-copy
";

/// Convert the connected action object into an activation script, when the
/// mapping is honest. `Ok(None)` means "no activation" — the plugin falls back
/// to calling the query program with `--activate`.
fn convert_action(
    action: &plist::Dictionary,
    source: &Path,
    directory: &Path,
    warnings: &mut Vec<String>,
) -> Result<Option<String>, Error> {
    let kind = action.get("type").and_then(Value::as_string).unwrap_or("");
    let config = action
        .get("config")
        .and_then(Value::as_dictionary)
        .cloned()
        .unwrap_or_default();

    let script = match kind {
        "alfred.workflow.action.openurl" => {
            // Never interpolated into the script: `$(…)` inside a double-quoted
            // bash string still executes, and a generated file that *looks*
            // like "open this URL" must not be able to run anything. The URL
            // goes into a sibling data file; the script is a fixed template
            // that substitutes the argument with bash's literal pattern
            // replacement — data stays data.
            let url = config.get("url").and_then(Value::as_string).unwrap_or("");
            std::fs::write(directory.join("open.data"), url)?;
            OPEN_URL_TEMPLATE.to_owned()
        }
        "alfred.workflow.action.script" => script_from(&config, source)?,
        "alfred.workflow.output.clipboard" => {
            warnings.push("the clipboard action needs `wl-copy` at run time".to_owned());
            let text = config
                .get("clipboardtext")
                .and_then(Value::as_string)
                .unwrap_or("{query}");
            std::fs::write(directory.join("open.data"), text)?;
            COPY_TEMPLATE.to_owned()
        }
        other => {
            warnings.push(format!(
                "action {other:?} has no jump equivalent; activation falls back to the query \
                 program with --activate"
            ));
            return Ok(None);
        }
    };

    Ok(Some(write_script(directory, "open", &script)?))
}

/// Write an executable script into the plugin directory, returning its name.
fn write_script(directory: &Path, name: &str, contents: &str) -> Result<String, Error> {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(name);
    std::fs::write(&path, contents)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(name.to_owned())
}

/// Carry the icon over: the filter's own (`<uid>.png`) wins over the
/// workflow's (`icon.png`). Returns the absolute destination path for the
/// manifest, which resolves regardless of the launcher's working directory.
fn copy_icon(uid: &str, source: &Path, directory: &Path) -> Result<Option<String>, Error> {
    for candidate in [format!("{uid}.png"), "icon.png".to_owned()] {
        let icon = source.join(&candidate);
        if icon.is_file() {
            let destination = directory.join("icon.png");
            std::fs::copy(&icon, &destination)?;
            return Ok(Some(destination.display().to_string()));
        }
    }
    Ok(None)
}

/// Extract an `.alfredworkflow` bundle (a zip) into a fresh private
/// directory, removed when the returned guard drops.
fn extract(bundle: &Path) -> Result<tempfile::TempDir, Error> {
    let available = std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join("bsdtar").is_file()));
    if !available {
        return Err(Error::NoExtractor);
    }

    let destination = tempfile::Builder::new().prefix("jump-alfred-").tempdir()?;

    let status = std::process::Command::new("bsdtar")
        .arg("-xf")
        .arg(bundle)
        .arg("-C")
        .arg(destination.path())
        .status()?;
    if !status.success() {
        return Err(Error::Io(std::io::Error::other(format!(
            "bsdtar exited with {status}"
        ))));
    }
    Ok(destination)
}

fn string_key(value: &Value, key: &str) -> Option<String> {
    value
        .as_dictionary()?
        .get(key)?
        .as_string()
        .map(ToOwned::to_owned)
}

/// A directory-safe name: lowercase alphanumerics and dashes, nothing else.
/// `/`, `.` and everything a path could be built from become dashes.
fn slugify(text: &str) -> String {
    let slug: String = text
        .to_lowercase()
        .replace(|character: char| !character.is_alphanumeric(), "-");
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "imported".to_owned()
    } else {
        slug
    }
}

fn toml_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but real info.plist: one bash script filter with a `{query}`
    /// placeholder, connected to an Open URL action, plus one modifier
    /// connection that must not be converted.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>name</key><string>GitHub Search</string>
  <key>description</key><string>Search repositories</string>
  <key>objects</key><array>
    <dict>
      <key>type</key><string>alfred.workflow.input.scriptfilter</string>
      <key>uid</key><string>filter-1</string>
      <key>title</key><string>Search GitHub</string>
      <key>config</key><dict>
        <key>keyword</key><string>gh</string>
        <key>scriptargtype</key><integer>0</integer>
        <key>type</key><integer>0</integer>
        <key>script</key><string>echo "{\"items\":[{\"title\":\"{query}\",\"arg\":\"{query}\"}]}"</string>
      </dict>
    </dict>
    <dict>
      <key>type</key><string>alfred.workflow.action.openurl</string>
      <key>uid</key><string>action-1</string>
      <key>config</key><dict>
        <key>url</key><string>https://github.com/search?q={query}</string>
      </dict>
    </dict>
  </array>
  <key>connections</key><dict>
    <key>filter-1</key><array>
      <dict><key>destinationuid</key><string>action-1</string><key>modifiers</key><integer>0</integer></dict>
      <dict><key>destinationuid</key><string>action-1</string><key>modifiers</key><integer>1048576</integer></dict>
    </array>
  </dict>
</dict></plist>"#;

    fn workspace(label: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("jump-alfred-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let workflow = root.join("workflow");
        let plugins = root.join("plugins");
        std::fs::create_dir_all(&workflow).expect("workflow dir");
        std::fs::create_dir_all(&plugins).expect("plugins dir");
        (workflow, plugins)
    }

    #[test]
    fn a_script_filter_becomes_a_runnable_plugin() {
        use std::os::unix::fs::PermissionsExt;

        let (workflow, plugins) = workspace("basic");
        std::fs::write(workflow.join("info.plist"), FIXTURE).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        assert_eq!(import.plugins.len(), 1);
        let plugin = &import.plugins[0];
        assert_eq!(plugin.keyword.as_deref(), Some("gh"));
        assert_eq!(plugin.name, "Search GitHub");

        // The manifest is one our own host parses.
        let manifest: jump_core::plugin::Manifest = toml::from_str(
            &std::fs::read_to_string(plugin.directory.join("manifest.toml")).expect("manifest"),
        )
        .expect("manifest parses");
        assert_eq!(manifest.keyword.as_deref(), Some("gh"));
        assert_eq!(manifest.timeout_ms, Some(3000));

        // The placeholder moved to argv, and the script is executable.
        let search = plugin.directory.join("search");
        let body = std::fs::read_to_string(&search).expect("script");
        assert!(body.starts_with("#!/usr/bin/env bash"));
        assert!(!body.contains("{query}"));
        assert!(body.contains("$1"));
        assert!(
            std::fs::metadata(&search)
                .expect("meta")
                .permissions()
                .mode()
                & 0o111
                != 0
        );

        // The Open URL action became a fixed xdg-open template; the URL lives
        // in a data file, never in the script.
        let open = std::fs::read_to_string(plugin.directory.join("open")).expect("open");
        assert!(open.contains("xdg-open"));
        assert!(!open.contains("github.com"));
        assert_eq!(
            std::fs::read_to_string(plugin.directory.join("open.data")).expect("data"),
            "https://github.com/search?q={query}"
        );

        // The modifier connection was reported, not silently eaten.
        assert!(
            import
                .warnings
                .iter()
                .any(|warning| warning.contains("modifier connections"))
        );
    }

    #[test]
    fn a_traversal_keyword_cannot_escape_the_plugin_root() {
        let (workflow, plugins) = workspace("traversal");
        let fixture = FIXTURE.replace("<string>gh</string>", "<string>../../escape</string>");
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        let directory = &import.plugins[0].directory;
        assert!(directory.starts_with(&plugins), "stayed inside the root");
        assert!(!plugins.parent().expect("parent").join("escape").exists());
    }

    #[test]
    fn a_hostile_url_never_reaches_the_shell() {
        let (workflow, plugins) = workspace("injection");
        let hostile = "https://x/$(touch /tmp/pwned)`id`";
        let fixture = FIXTURE.replace("https://github.com/search?q={query}", hostile);
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        let directory = &import.plugins[0].directory;

        // The script is the fixed template; every hostile byte is in the data
        // file, where bash's literal pattern replacement treats it as data.
        let open = std::fs::read_to_string(directory.join("open")).expect("open");
        assert!(!open.contains("touch"));
        assert!(!open.contains('`'));
        assert_eq!(
            std::fs::read_to_string(directory.join("open.data")).expect("data"),
            hostile
        );
    }

    #[test]
    fn importing_twice_refuses_to_overwrite() {
        let (workflow, plugins) = workspace("twice");
        std::fs::write(workflow.join("info.plist"), FIXTURE).expect("fixture");

        import(&workflow, &plugins).expect("first import");
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::AlreadyExists(_))
        ));
    }

    #[test]
    fn applescript_filters_are_refused_with_a_reason() {
        let (workflow, plugins) = workspace("osascript");
        let fixture = FIXTURE.replace(
            "<key>type</key><integer>0</integer>",
            "<key>type</key><integer>6</integer>",
        );
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        // The only filter is unconvertible, so the import as a whole fails —
        // but names the reason instead of producing an empty success.
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NoScriptFilters)
        ));
    }

    #[test]
    fn a_directory_without_info_plist_is_not_a_workflow() {
        let (workflow, plugins) = workspace("empty");
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NotAWorkflow)
        ));
    }
}
