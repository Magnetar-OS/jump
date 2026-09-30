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

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

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
    #[error("{0:?} is larger than a workflow file can be ({limit} MiB per file, {total} MiB per import)", limit = MAX_FILE_BYTES >> 20, total = MAX_IMPORT_BYTES >> 20)]
    TooLarge(String),
}

/// Largest single file read out of a workflow: its `info.plist`, a script,
/// an icon. Real ones are kilobytes.
const MAX_FILE_BYTES: u64 = 8 << 20;

/// Total read out of one workflow, however many filters it declares.
const MAX_IMPORT_BYTES: u64 = 32 << 20;

/// The workflow being imported: an extracted directory or an
/// `.alfredworkflow` archive, read one named file at a time.
///
/// Nothing is ever extracted wholesale. The importer needs `info.plist` plus
/// the scripts and icons it names, so those are all it reads, and every read
/// is capped — per file and across the import — while the bytes arrive,
/// never after. An archive's own size headers are not trusted: a zip bomb
/// is cut off at the cap however small it claims to be.
struct Bundle {
    source: BundleSource,
    /// Bytes still allowed across the whole import.
    budget: Cell<u64>,
}

enum BundleSource {
    /// A directory, canonicalised.
    Directory(PathBuf),
    /// A zip, read with `bsdtar`.
    Archive(PathBuf),
}

impl Bundle {
    fn open(workflow: &Path) -> Result<Self, Error> {
        let source = if workflow.is_dir() {
            BundleSource::Directory(workflow.canonicalize()?)
        } else {
            let available = std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths).any(|dir| dir.join("bsdtar").is_file())
            });
            if !available {
                return Err(Error::NoExtractor);
            }
            BundleSource::Archive(workflow.to_path_buf())
        };
        Ok(Self {
            source,
            budget: Cell::new(MAX_IMPORT_BYTES),
        })
    }

    /// The contents of `relative`, or `None` when the bundle has no regular
    /// file there.
    ///
    /// Every path an untrusted plist names goes through here. In a directory
    /// it is resolved first, so `../`, an absolute path and a symlink out of
    /// the bundle are all the same refused read, and only a regular file is
    /// read — `/dev/zero` or a FIFO must not turn an import into an endless
    /// read. In an archive only a plain relative member name is looked up.
    fn read(&self, relative: &str) -> Result<Option<Vec<u8>>, Error> {
        let limit = MAX_FILE_BYTES.min(self.budget.get());
        let contents = match &self.source {
            BundleSource::Directory(root) => {
                let Ok(path) = root.join(relative).canonicalize() else {
                    return Ok(None);
                };
                if !path.starts_with(root) || !path.is_file() {
                    return Ok(None);
                }
                let mut contents = Vec::new();
                std::fs::File::open(path)?
                    .take(limit + 1)
                    .read_to_end(&mut contents)?;
                contents
            }
            BundleSource::Archive(archive) => {
                let Some(member) = archive_member(relative) else {
                    return Ok(None);
                };
                match read_member(archive, member, limit)? {
                    Some(contents) => contents,
                    None => return Ok(None),
                }
            }
        };
        let length = contents.len() as u64;
        if length > limit {
            return Err(Error::TooLarge(relative.to_owned()));
        }
        self.budget.set(self.budget.get() - length);
        Ok(Some(contents))
    }
}

/// `relative` as an archive member name, when it is a plain relative path.
///
/// `bsdtar` matches its arguments as patterns, so a name with pattern
/// characters could select other members; such names, absolute paths, `..`
/// and anything that could read as an option are refused outright.
fn archive_member(relative: &str) -> Option<&str> {
    let relative = relative.strip_prefix("./").unwrap_or(relative);
    let plain = !relative.is_empty()
        && !relative.starts_with('-')
        && !relative.contains(['*', '?', '[', ']', '\\'])
        && Path::new(relative)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    plain.then_some(relative)
}

/// Stream one member of `archive` through `bsdtar -O`, stopping at
/// `limit + 1` bytes. `None` when the archive has no such member.
fn read_member(archive: &Path, member: &str, limit: u64) -> Result<Option<Vec<u8>>, Error> {
    let mut child = std::process::Command::new("bsdtar")
        .arg("-xOf")
        .arg(archive)
        .arg(member)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let mut contents = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("stdout is piped")
        .take(limit + 1)
        .read_to_end(&mut contents);
    if read.is_err() || contents.len() as u64 > limit {
        // Over the cap: the rest is never decompressed.
        let _ = child.kill();
    }
    let status = child.wait()?;
    read?;
    if contents.len() as u64 > limit {
        return Ok(Some(contents));
    }
    match (status.success(), contents.is_empty()) {
        (true, _) => Ok(Some(contents)),
        // bsdtar's answer for a name the archive does not have.
        (false, true) => Ok(None),
        (false, false) => Err(Error::Io(std::io::Error::other(format!(
            "bsdtar exited with {status} reading {member:?}"
        )))),
    }
}

/// Import `workflow` — an `.alfredworkflow` bundle or an extracted directory —
/// creating one plugin per script filter under `destination_root`.
pub fn import(workflow: &Path, destination_root: &Path) -> Result<Import, Error> {
    let bundle = Bundle::open(workflow)?;
    let info = bundle.read("info.plist")?.ok_or(Error::NotAWorkflow)?;
    let info = Value::from_reader(std::io::Cursor::new(info))?;

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

    let planned = plan(
        filters,
        destination_root,
        &workflow_name,
        &mut import.warnings,
    )?;

    // Each plugin is built in a private staging directory beside its
    // destination and moved into place only once every filter has been
    // converted, so a filter that fails half-way — or an import that fails
    // as a whole, even while moving — leaves nothing behind in the plugin
    // root.
    std::fs::create_dir_all(destination_root)?;
    let mut staged = Vec::new();
    for (uid, filter, keyword, directory) in planned {
        let staging = tempfile::Builder::new()
            .prefix(".jump-import-")
            .tempdir_in(destination_root)?;
        match convert_filter(
            uid,
            filter,
            keyword,
            &objects,
            connections,
            &bundle,
            staging.path(),
            &directory,
            &workflow_name,
            &workflow_description,
            &mut import.warnings,
        ) {
            Ok(imported) => staged.push((staging, imported)),
            // A bundle over its size limits is refused as a whole, not
            // filter by filter.
            Err(error @ Error::TooLarge(_)) => return Err(error),
            Err(error) => import
                .warnings
                .push(format!("script filter {uid} skipped: {error}")),
        }
    }

    if staged.is_empty() {
        return Err(Error::NoScriptFilters);
    }
    import.plugins = place(staged)?;
    Ok(import)
}

/// Move every staged plugin into place, or none of them.
///
/// There is no renaming several directories at once, so a move that fails
/// after earlier ones succeeded is undone: each plugin already in place goes
/// back to its staging directory, whose guard then removes it. The staging
/// directories sit beside their destinations, on the same filesystem, so
/// both directions are plain renames.
fn place(staged: Vec<(tempfile::TempDir, Imported)>) -> Result<Vec<Imported>, Error> {
    let mut placed: Vec<(tempfile::TempDir, Imported)> = Vec::new();
    for (staging, imported) in staged {
        if let Err(error) = std::fs::rename(staging.path(), &imported.directory) {
            for (staging, imported) in &placed {
                take_back(staging.path(), &imported.directory)?;
            }
            return Err(error.into());
        }
        placed.push((staging, imported));
    }
    Ok(placed
        .into_iter()
        .map(|(staging, imported)| {
            // Moved for good, so there is nothing left for the guard to
            // remove.
            let _ = staging.keep();
            imported
        })
        .collect())
}

/// Undo one placed plugin: back to `staging`, or deleted where it stands if
/// even that fails. An error only when it could be neither moved nor
/// deleted, which names the directory the import had to leave behind.
fn take_back(staging: &Path, directory: &Path) -> Result<(), Error> {
    if std::fs::rename(directory, staging).is_ok() {
        return Ok(());
    }
    std::fs::remove_dir_all(directory).map_err(|error| {
        Error::Io(std::io::Error::other(format!(
            "the import failed and {} could not be removed again: {error}",
            directory.display()
        )))
    })
}

/// A script filter and the plugin directory it will become.
type Planned<'a> = (&'a String, &'a plist::Dictionary, Option<String>, PathBuf);

/// Decide every filter's destination, before anything is written.
///
/// A collision with an existing directory aborts the whole import: partial
/// output plus an error is the worst of both worlds. Two filters of one
/// workflow landing on the same name — a shared keyword, or two untitled
/// keywordless filters — are both kept, the later one numbered.
fn plan<'a>(
    filters: Vec<(&'a String, &&'a plist::Dictionary)>,
    destination_root: &Path,
    workflow_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<Planned<'a>>, Error> {
    let mut planned: Vec<Planned<'a>> = Vec::new();
    for (uid, filter) in filters {
        let keyword = filter_keyword(filter, warnings);
        let title = filter_title(filter, workflow_name);
        // The keyword comes straight out of an untrusted plist, so only its
        // slug names the directory — `../../.config/...` must not write
        // outside the plugin root. The manifest keeps the declared text.
        let base = slugify(keyword.as_deref().unwrap_or(&title));
        let mut slug = base.clone();
        let mut suffix = 2;
        while planned
            .iter()
            .any(|(_, _, _, directory)| *directory == destination_root.join(&slug))
        {
            slug = format!("{base}-{suffix}");
            suffix += 1;
        }
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
        planned.push((uid, *filter, keyword, directory));
    }
    Ok(planned)
}

/// The filter's keyword, or `None` for an always-on filter.
fn filter_keyword(filter: &plist::Dictionary, warnings: &mut Vec<String>) -> Option<String> {
    let keyword = filter
        .get("config")
        .and_then(Value::as_dictionary)
        .and_then(|config| config.get("keyword"))
        .and_then(Value::as_string)
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(ToOwned::to_owned);

    // Alfred allows `{var:…}` templates in keywords; jump keywords are
    // literal. The plugin still imports — as an always-on plugin — but the
    // difference is worth a warning.
    // Nor can a keyword with a space in it ever match.
    match keyword {
        Some(keyword) if keyword.contains('{') => {
            warnings.push(format!(
                "keyword {keyword:?} is templated; imported without a keyword — set one in \
                 manifest.toml"
            ));
            None
        }
        Some(keyword) if !jump_core::plugin::is_keyword(&keyword) => {
            warnings.push(format!(
                "keyword {keyword:?} is not one word; imported without a keyword — set one in \
                 manifest.toml"
            ));
            None
        }
        other => other,
    }
}

/// The filter's own title, else the workflow's name.
fn filter_title(filter: &plist::Dictionary, workflow_name: &str) -> String {
    filter
        .get("title")
        .and_then(Value::as_string)
        .filter(|title| !title.is_empty())
        .unwrap_or(workflow_name)
        .to_owned()
}

/// Convert one script filter and its primary connection, writing the plugin
/// into `staging`. `directory` is where it will live once moved into place,
/// which is what the manifest's absolute icon path has to name.
#[allow(clippy::too_many_arguments)]
fn convert_filter(
    uid: &str,
    filter: &plist::Dictionary,
    keyword: Option<String>,
    objects: &HashMap<String, &plist::Dictionary>,
    connections: Option<&plist::Dictionary>,
    bundle: &Bundle,
    staging: &Path,
    directory: &Path,
    workflow_name: &str,
    workflow_description: &str,
    warnings: &mut Vec<String>,
) -> Result<Imported, Error> {
    let config = filter
        .get("config")
        .and_then(Value::as_dictionary)
        .cloned()
        .unwrap_or_default();

    let title = filter_title(filter, workflow_name);

    let query = script_from(&config, bundle)?;
    let query_file = write_script(staging, "search", &query)?;

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
        .and_then(|action| convert_action(action, bundle, staging, warnings).transpose())
        .transpose()?;

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
    if let Some(icon) = copy_icon(uid, bundle, staging, directory)? {
        let _ = writeln!(manifest, "icon = \"{}\"", toml_escape(&icon));
    }
    // Alfred workflows were written with no deadline at all, so a keyworded
    // filter gets the ceiling rather than the 180 ms interactive default. An
    // always-on one cannot: it runs on every keystroke, and the host holds
    // it to the default whatever the manifest says.
    if keyword.is_some() {
        manifest.push_str("# Imported from Alfred, which has no query deadline; tune down once\n");
        manifest.push_str("# you know how fast it answers.\ntimeout_ms = 3000\n");
    } else {
        manifest.push_str("# No keyword, so this runs on every keystroke and must answer within\n");
        manifest.push_str("# 180 ms. Give it a keyword to allow a timeout_ms of up to 3000.\n");
    }
    std::fs::write(staging.join("manifest.toml"), manifest)?;

    Ok(Imported {
        directory: directory.to_path_buf(),
        name: title,
        keyword,
    })
}

/// The filter's query program, as (shebang-prefixed) script text.
fn script_from(config: &plist::Dictionary, bundle: &Bundle) -> Result<String, Error> {
    // `scriptfile` wins when both are present, matching Alfred.
    if let Some(file) = config
        .get("scriptfile")
        .and_then(Value::as_string)
        .filter(|file| !file.is_empty())
    {
        let bytes = bundle.read(file)?.ok_or_else(|| {
            Error::Io(std::io::Error::other(format!(
                "script file {file:?} is not a file inside the workflow"
            )))
        })?;
        let text = String::from_utf8(bytes).map_err(|_| {
            Error::Io(std::io::Error::other(format!(
                "script file {file:?} is not UTF-8 text"
            )))
        })?;
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
    bundle: &Bundle,
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
        "alfred.workflow.action.script" => script_from(&config, bundle)?,
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

/// Carry the icon over into `staging`: the filter's own (`<uid>.png`) wins
/// over the workflow's (`icon.png`). Returns the absolute path the icon will
/// have once the plugin is in `directory`, which resolves regardless of the
/// launcher's working directory.
fn copy_icon(
    uid: &str,
    bundle: &Bundle,
    staging: &Path,
    directory: &Path,
) -> Result<Option<String>, Error> {
    for candidate in [format!("{uid}.png"), "icon.png".to_owned()] {
        if let Some(icon) = bundle.read(&candidate)? {
            std::fs::write(staging.join("icon.png"), icon)?;
            return Ok(Some(directory.join("icon.png").display().to_string()));
        }
    }
    Ok(None)
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

    /// A private scratch area per test. The guard must stay bound for the
    /// test's lifetime — the directory vanishes when it drops. Same `tempfile`
    /// the production path uses, so the test file does not demonstrate the
    /// exact pattern the import path exists to avoid.
    fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::Builder::new()
            .prefix("jump-alfred-test-")
            .tempdir()
            .expect("test workspace");
        let workflow = root.path().join("workflow");
        let plugins = root.path().join("plugins");
        std::fs::create_dir_all(&workflow).expect("workflow dir");
        std::fs::create_dir_all(&plugins).expect("plugins dir");
        (root, workflow, plugins)
    }

    #[test]
    fn a_script_filter_becomes_a_runnable_plugin() {
        use std::os::unix::fs::PermissionsExt;

        let (_root, workflow, plugins) = workspace();
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
    fn an_always_on_filter_keeps_the_interactive_deadline() {
        let (_root, workflow, plugins) = workspace();
        let fixture = FIXTURE.replace("<key>keyword</key><string>gh</string>", "");
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        let plugin = &import.plugins[0];
        assert_eq!(plugin.keyword, None);
        let manifest: jump_core::plugin::Manifest = toml::from_str(
            &std::fs::read_to_string(plugin.directory.join("manifest.toml")).expect("manifest"),
        )
        .expect("manifest parses");
        assert_eq!(manifest.timeout_ms, None);
    }

    #[test]
    fn a_keyword_that_could_never_match_is_not_imported_as_one() {
        let (_root, workflow, plugins) = workspace();
        let fixture = FIXTURE.replace("<string>gh</string>", "<string>gh search</string>");
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        assert_eq!(import.plugins[0].keyword, None);
        assert!(
            import
                .warnings
                .iter()
                .any(|warning| warning.contains("not one word"))
        );
    }

    #[test]
    fn a_traversal_keyword_cannot_escape_the_plugin_root() {
        let (_root, workflow, plugins) = workspace();
        let fixture = FIXTURE.replace("<string>gh</string>", "<string>../../escape</string>");
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        let directory = &import.plugins[0].directory;
        assert!(directory.starts_with(&plugins), "stayed inside the root");
        assert!(!plugins.parent().expect("parent").join("escape").exists());
    }

    #[test]
    fn a_hostile_url_never_reaches_the_shell() {
        let (_root, workflow, plugins) = workspace();
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
        let (_root, workflow, plugins) = workspace();
        std::fs::write(workflow.join("info.plist"), FIXTURE).expect("fixture");

        import(&workflow, &plugins).expect("first import");
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::AlreadyExists(_))
        ));
    }

    #[test]
    fn applescript_filters_are_refused_with_a_reason() {
        let (_root, workflow, plugins) = workspace();
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

    /// [`FIXTURE`] plus a second, unconnected script filter with `keyword`.
    fn with_second_filter(keyword: &str) -> String {
        FIXTURE.replace(
            "  </array>\n  <key>connections</key>",
            &format!(
                "    <dict>
      <key>type</key><string>alfred.workflow.input.scriptfilter</string>
      <key>uid</key><string>filter-2</string>
      <key>config</key><dict>
        <key>keyword</key><string>{keyword}</string>
        <key>type</key><integer>0</integer>
        <key>script</key><string>echo '{{\"items\":[]}}'</string>
      </dict>
    </dict>
  </array>
  <key>connections</key>"
            ),
        )
    }

    fn entries(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_collision_aborts_before_anything_is_written() {
        let (_root, workflow, plugins) = workspace();
        std::fs::write(workflow.join("info.plist"), with_second_filter("other")).expect("fixture");
        std::fs::create_dir_all(plugins.join("other")).expect("existing plugin");

        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::AlreadyExists(_))
        ));
        // filter-1 sorts first; it must not have been written either.
        assert_eq!(entries(&plugins), ["other"]);
    }

    #[test]
    fn a_move_that_fails_undoes_the_ones_before_it() {
        let (_root, _workflow, plugins) = workspace();
        let stage = |name: &str| {
            let staging = tempfile::Builder::new()
                .prefix(".jump-import-")
                .tempdir_in(&plugins)
                .expect("staging");
            std::fs::write(staging.path().join("manifest.toml"), "name = \"x\"\n")
                .expect("manifest");
            let imported = Imported {
                directory: plugins.join(name),
                name: name.to_owned(),
                keyword: None,
            };
            (staging, imported)
        };
        let staged = vec![stage("first"), stage("second")];
        // Something took the second name after the import had planned around
        // it, and a directory with contents cannot be renamed over.
        std::fs::create_dir_all(plugins.join("second")).expect("squatter");
        std::fs::write(plugins.join("second").join("theirs"), "kept").expect("squatter file");

        assert!(matches!(place(staged), Err(Error::Io(_))));

        // The first plugin had already moved into place; it is gone again,
        // no staging directory is left, and what was there is untouched.
        assert_eq!(entries(&plugins), ["second"]);
        assert_eq!(
            std::fs::read_to_string(plugins.join("second").join("theirs")).expect("squatter"),
            "kept"
        );
    }

    #[test]
    fn filters_sharing_a_keyword_both_import() {
        let (_root, workflow, plugins) = workspace();
        std::fs::write(workflow.join("info.plist"), with_second_filter("gh")).expect("fixture");

        let import = import(&workflow, &plugins).expect("import succeeds");
        assert_eq!(import.plugins.len(), 2);
        assert_eq!(entries(&plugins), ["gh", "gh-2"]);
        // Both keep the keyword the workflow declared; only the directory
        // name had to differ.
        assert!(
            import
                .plugins
                .iter()
                .all(|plugin| plugin.keyword.as_deref() == Some("gh"))
        );
    }

    #[test]
    fn a_skipped_filter_leaves_nothing_behind() {
        let (_root, workflow, plugins) = workspace();
        // The filter converts, but the Run Script it feeds is AppleScript.
        let fixture = FIXTURE
            .replace(
                "alfred.workflow.action.openurl",
                "alfred.workflow.action.script",
            )
            .replace(
                "<key>url</key><string>https://github.com/search?q={query}</string>",
                "<key>type</key><integer>6</integer><key>script</key><string>x</string>",
            );
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");

        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NoScriptFilters)
        ));
        assert!(entries(&plugins).is_empty(), "{:?}", entries(&plugins));
    }

    #[test]
    fn a_scriptfile_outside_the_bundle_is_never_read() {
        let (root, workflow, plugins) = workspace();
        let secret = root.path().join("secret");
        std::fs::write(&secret, "not for the plugin").expect("secret");
        let fixture = FIXTURE.replace(
            "<key>scriptargtype</key>",
            &format!(
                "<key>scriptfile</key><string>{}</string><key>scriptargtype</key>",
                secret.display()
            ),
        );
        std::fs::write(workflow.join("info.plist"), &fixture).expect("fixture");
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NoScriptFilters)
        ));

        // A relative escape and a symlink out of the bundle are the same
        // read by another spelling.
        let relative = fixture.replace(&secret.display().to_string(), "../secret");
        std::fs::write(workflow.join("info.plist"), relative).expect("fixture");
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NoScriptFilters)
        ));
        std::os::unix::fs::symlink(&secret, workflow.join("link.sh")).expect("symlink");
        let linked = fixture.replace(&secret.display().to_string(), "link.sh");
        std::fs::write(workflow.join("info.plist"), linked).expect("fixture");
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NoScriptFilters)
        ));

        assert!(entries(&plugins).is_empty());
    }

    /// Zip `members` of `directory` into an `.alfredworkflow` beside it,
    /// named the way Alfred names them: no `./` prefix.
    fn archive(directory: &Path, members: &[&str]) -> PathBuf {
        let bundle = directory.with_extension("alfredworkflow");
        let status = std::process::Command::new("bsdtar")
            .args(["--format", "zip", "-cf"])
            .arg(&bundle)
            .arg("-C")
            .arg(directory)
            .args(members)
            .status()
            .expect("bsdtar is installed");
        assert!(status.success());
        bundle
    }

    #[test]
    fn an_archived_workflow_imports() {
        let (_root, workflow, plugins) = workspace();
        let fixture = FIXTURE.replace(
            "<key>scriptargtype</key>",
            "<key>scriptfile</key><string>filter.sh</string><key>scriptargtype</key>",
        );
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");
        std::fs::write(workflow.join("filter.sh"), "#!/bin/sh\necho \"{query}\"\n")
            .expect("script");
        std::fs::write(workflow.join("icon.png"), b"png").expect("icon");
        let bundle = archive(&workflow, &["info.plist", "filter.sh", "icon.png"]);

        let import = import(&bundle, &plugins).expect("import succeeds");
        let directory = &import.plugins[0].directory;
        assert_eq!(
            std::fs::read_to_string(directory.join("search")).expect("script"),
            "#!/bin/sh\necho \"$1\"\n"
        );
        assert_eq!(
            std::fs::read(directory.join("icon.png")).expect("icon"),
            b"png"
        );
    }

    #[test]
    fn a_zip_bomb_is_cut_off_at_the_cap() {
        // 64 MiB of one byte compresses to next to nothing. Extracting the
        // bundle wholesale wrote all of it to /tmp — RAM, on tmpfs — before
        // anything looked at a size; now the read stops at the cap.
        let (_root, workflow, plugins) = workspace();
        let fixture = FIXTURE.replace(
            "<key>scriptargtype</key>",
            "<key>scriptfile</key><string>bomb.sh</string><key>scriptargtype</key>",
        );
        std::fs::write(workflow.join("info.plist"), fixture).expect("fixture");
        std::fs::write(workflow.join("bomb.sh"), vec![b'a'; 64 << 20]).expect("bomb");
        let bundle = archive(&workflow, &["info.plist", "bomb.sh"]);
        assert!(std::fs::metadata(&bundle).expect("bundle").len() < 1 << 20);

        assert!(matches!(
            import(&bundle, &plugins),
            Err(Error::TooLarge(name)) if name == "bomb.sh"
        ));
        assert!(entries(&plugins).is_empty(), "{:?}", entries(&plugins));
    }

    #[test]
    fn archive_members_must_be_plain_relative_names() {
        assert_eq!(archive_member("info.plist"), Some("info.plist"));
        assert_eq!(archive_member("./bin/run.sh"), Some("bin/run.sh"));
        for hostile in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "*",
            "icon?.png",
            "-x",
        ] {
            assert_eq!(archive_member(hostile), None, "{hostile:?}");
        }
    }

    #[test]
    fn a_directory_without_info_plist_is_not_a_workflow() {
        let (_root, workflow, plugins) = workspace();
        assert!(matches!(
            import(&workflow, &plugins),
            Err(Error::NotAWorkflow)
        ));
    }
}
