# COSMIC application conventions

**Field notes · merged edition · August 2026 · libcosmic `ef490df5` · COSMIC 1.5.0**

What System76's desktop repositories agree on, read off the source rather than off documentation that mostly does not exist. Each convention says where it was seen, whether it was measured on a live session, and how three third-party projects decided to follow it or not:

- **grabit** — selection-triggered action bar · daemon, three layer surfaces, no main window
- **Locket** — application suite with an applet · windowed, nav bar, keybinds, D-Bus
- **Peek** — document viewer with a layer-shell overlay · resident, single-instance, blur region

plus a general survey of eight repositories on a live COSMIC 1.5.0 session and pop-launcher 1.2.7.

**Legend** — `seen in:` the checkouts a pattern appears in · `measured:` reproduced on a running desktop, not inferred · per-project status: **follows** / **partial** / **gap** / **rejected** (a deliberate divergence, with the reason).

---

## Contents

- [Sources](#sources)
- **Project** — [Dependencies & toolchain](#dependencies--toolchain) · [Repository layout](#repository-layout) · [The justfile](#the-justfile)
- **Metadata** — [Identity](#identity-one-reverse-dns-id-everything-named-after-it) · [Generated XDG files](#generated-xdg-files-xdgen) · [Desktop entry & AppStream](#desktop-entry--appstream) · [Localisation](#localisation)
- **Runtime** — [The Application shape](#the-cosmicapplication-shape) · [Configuration](#configuration-cosmic-config) · [Theme & spacing](#theme--spacing) · [Icons](#icons) · [Panel applets](#panel-applets) · [Layer-shell surfaces](#layer-shell-surfaces) · [The pointer problem](#the-pointer-problem) · [Launching applications](#launching-applications)
- **Practice** — [Shared components & gaps](#shared-components-worth-reaching-for-and-gaps-in-libcosmic) · [Where it disagrees](#where-the-ecosystem-does-not-agree) · [Adoption across projects](#adoption-across-the-three-projects) · [Checklist](#checklist-for-a-new-cosmic-application)

---

## Sources

Every claim below comes from one of these checkouts or from a measurement against the running desktop. All were current on 24 August 2026. The two `cosmic-*-template` repositories are the normative source; where a shipped app diverges from them it is usually solving a problem the template does not have.

| Repository | Revision | Why it mattered |
|---|---|---|
| pop-os/libcosmic | `ef490df5` | The toolkit; layer-surface, popup and applet APIs; re-exports cosmic-config |
| pop-os/cosmic-launcher | `a9ad093` | On-demand overlay; launches apps; the newest surface API; 74 languages |
| pop-os/cosmic-app-library | `385db1e` | Second overlay; app grid; the other half of the launch sequence |
| pop-os/cosmic-panel | `3c08c30` | Long-lived layer surfaces; hosts applets as a nested compositor |
| pop-os/cosmic-edit | `0e9c927` | A full windowed application; complete metainfo; syntax theming |
| pop-os/cosmic-app-template | `97ff759` | The canonical skeleton; xdgen build.rs |
| pop-os/cosmic-applet-template | `58f506f` | Applet variant of the same |
| cosmic-utils/cosmic-ext-camera | `7520175` | A community app; correct metainfo path; wgpu primitives for video |
| pop-launcher | 1.2.7 (installed) | Driven directly over stdio to confirm two undocumented behaviours |
| cosmic-comp | live 1.5.0 session | Pointer-focus and systemd-scope behaviour, with `WAYLAND_DEBUG=1` |

Most COSMIC applications are `cargo generate`d from one of the two templates and keep the skeleton verbatim. That is why the conventions hold so tightly, and why the places they break down are informative.

---

## Dependencies & toolchain

### Track libcosmic's default branch; pin nothing but Cargo.lock

*Seen in: all eight repos · grabit follows · Locket follows · Peek follows*

libcosmic has no crates.io release. Every project takes it as a git dependency with no `rev` — not one pins — and commits `Cargo.lock`, libraries included. The lockfile is the reproducibility mechanism; `cargo update -p libcosmic` is the deliberate update.

```toml
[dependencies.libcosmic]
git = "https://github.com/pop-os/libcosmic.git"
features = [
    # One comment per feature, saying why it is on.
    "about",
    "single-instance",
    "wgpu",
    "xdg-portal",
]

# Uncomment to test against a locally-cloned libcosmic
# [patch.'https://github.com/pop-os/libcosmic']
# libcosmic = { path = "../libcosmic" }
# cosmic-config = { path = "../libcosmic/cosmic-config" }
```

> **⚠ A rev costs more than it looks.** libcosmic depends on `cosmic-panel-config` and `cosmic-settings-config`, which themselves follow libcosmic's branch. Pinning an older libcosmic makes cargo compile two copies of `cosmic-config`, `iced_core` and `iced_futures` — one for the pin, one for the branch.

- **Do not depend on cosmic-config separately.** libcosmic re-exports it as `cosmic::cosmic_config`, and `macro` is one of its default features, so the derive arrives with it: `cosmic::cosmic_config::cosmic_config_derive::CosmicConfigEntry`.
- Features are selected explicitly. Two styles exist — an inline array (cosmic-launcher, cosmic-app-library) or a `[dependencies.libcosmic]` section with one comment per feature (cosmic-ext-camera, the template). The commented form is considerably easier to review.
- Tokio is the executor, behind the `tokio` feature.

### Toolchain pinned in one place

*Seen in: libcosmic, cosmic-edit, cosmic-app-library, cosmic-launcher, cosmic-panel · Locket follows · Peek partial · grabit follows*

`edition = "2024"`, with `rust-version = "1.93"` across libcosmic, cosmic-edit and cosmic-app-library — conservative and shared, so one distro toolchain builds all of them. Where it appears, `rust-toolchain.toml` names the same channel and its components; rustup reads it with no extra tooling, and CI gets it for free if the workflow simply does not pass a toolchain to an action. `rustfmt.toml` exists in about half the repositories and is one line: `imports_granularity = "Module"`.

> **⚠ They move together.** `rust-version` in Cargo.toml and the channel in `rust-toolchain.toml` have to agree. When they drift, cargo refuses to build at all.

Peek declares 1.93 to match but has neither a `rust-toolchain.toml` nor a `rustfmt.toml`; the second would reformat every import across the tree, which is a separate decision.

### Licensing

*Seen in: cosmic-edit, cosmic-ext-camera, libcosmic · Peek partial*

Every application in the set is `GPL-3.0-only`; libcosmic itself is `MPL-2.0` so it can be linked from anywhere. Source files open with a copyright line and an `SPDX-License-Identifier` comment. Peek is `GPL-3.0-or-later` rather than `-only` because poppler forces GPL on the whole workspace including the engine crate — a deliberate divergence, not a drift.

---

## Repository layout

*Seen in: all eight repos · grabit follows · Locket follows · Peek follows*

Every project lands in the same shape, closely enough that you can navigate an unfamiliar one immediately.

```
justfile                build and install entry point; metadata checks
Cargo.lock              committed, including for libraries
rust-toolchain.toml     channel + components
rustfmt.toml            usually imports_granularity = "Module"
i18n.toml               fallback_language + assets_dir — four lines, identical everywhere
i18n/<lang>/<crate>.ftl one catalogue per locale; the filename is the crate name
src/
  main.rs               localisation init, then hand off to cosmic
  app.rs                the cosmic::Application impl and its Message enum
  config.rs             one CosmicConfigEntry struct
  i18n.rs               (template) or localize.rs (shipped apps) — the fl! macro
build.rs                generates desktop entry and metainfo from fluent strings (templates)
data/ | res/ | resources/   desktop entry, metainfo, icons; data/justfile in first-party apps
debian/  flake.nix  hooks/pre-commit.hook  .github/     packaging; arrive when packaging starts
```

- **The resource directory has no agreed name.** `resources/` in both templates and cosmic-ext-camera, `data/` in cosmic-launcher and cosmic-app-library, `res/` in cosmic-edit. Where templates are expanded at build time, `resources/` holds templates and `data/` finished files. Pick one and be consistent inside your own repo — the install destinations are identical regardless.
- `src/app.rs` is expected to be large — cosmic-app-library's is 1903 lines, cosmic-launcher's 1290, each with a single `update` match arm per message. Splitting the view into `view.rs` is common; splitting `update` is not. `#[allow(clippy::too_many_lines)]` on `update` is the accepted answer.
- `debian/` (control, rules, install, copyright, changelog), `flake.nix` and a `hooks/pre-commit.hook` that runs `cargo fmt --check` are in every shipping app and neither template. They are packaging choices, not framework conventions — they arrive when a project starts being packaged. None of the three projects has them yet.

grabit reduced `data/` to a systemd unit, since everything else is generated, plus a `gnome-extension/` directory that has no COSMIC equivalent.

---

## The justfile

*Seen in: all eight repos · grabit follows · Locket follows · Peek follows*

Every project uses `just`, never make, with a preamble and recipe set copied nearly verbatim between them. A packager who has built one COSMIC application can build all of them without reading the file.

```just
export NAME  := 'cosmic-launcher'
export APPID := 'com.system76.CosmicLauncher'

rootdir := ''
prefix  := '/usr'
debug   := '0'

base-dir         := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-src := cargo-target-dir / (if debug == '1' { 'debug' } else { 'release' }) / NAME
bin-dst := base-dir / 'bin' / NAME

default: build-release
clean:                   # cargo clean
clean-vendor:            # rm -rf .cargo vendor vendor.tar
clean-dist: clean clean-vendor
build-debug *args:       # cargo build --locked
build-release *args: (build-debug '--release' args)
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)
check *args:             # cargo clippy --all-features --locked -- -W clippy::pedantic
run *args:
vendor:                  # cargo vendor --sync, then tar it, honouring SOURCE_DATE_EPOCH
vendor-extract:          # tar pxf vendor.tar
install:                 # install -Dm0755 the binary, -Dm0644 the data, then refresh caches
uninstall:
```

The parts that carry weight:

- **`rootdir` is the staging root** — the DESTDIR equivalent. Every destination derives from it, so `just rootdir=/tmp/stage install` produces a complete tree without touching the system, and a packager runs `just rootdir=$pkgdir install`. Never hardcode `/usr`.
- **Install wires nothing up.** Anything that takes a bus name, enables a unit or edits a PAM stack belongs in an interactive script the user runs knowingly. Installing files and activating them are separate acts.
- **`cargo-target-dir` reads the environment** rather than hardcoding `target/`. Anything a build script emits has to honour the same variable, or the install recipes will not find it — see the xdgen note below.
- **`NAME` and `APPID` are exported**, so nested justfiles inherit them. cosmic-launcher's root `install` is three lines delegating to `data/justfile` and `data/icons/justfile`.
- **`clippy::pedantic` as warnings, not denials**, is the project standard. Pedantic flags every numeric cast and UI geometry code is full of them; nothing in the ecosystem treats a pedantic lint as a build failure. grabit's `just check` uses `-D warnings`, which is stricter than any COSMIC project.
- **`vendor` and `build-vendored`** exist for distribution packaging: with a git dependency, a packager with no network needs the tarball. If you add another git dependency, `just vendor` must still work. The linker preamble picks up clang and mold when both are present.

### Install destinations, and the two caches that must be poked

| Artifact | Destination |
|---|---|
| binary | `{prefix}/bin/{name}` |
| desktop entry | `{prefix}/share/applications/{appid}.desktop` |
| AppStream metainfo | `{prefix}/share/metainfo/{appid}.metainfo.xml` |
| icon | `{prefix}/share/icons/hicolor/scalable/apps/{appid}.svg` (see Identity for the per-size variant) |
| user unit | `{prefix}/lib/systemd/user/{name}.service` |

Then `update-desktop-database` and `gtk-update-icon-cache`. Both are caches; neither notices a new file on its own, and "Open With" reads the first of them.

> **⚠ Legacy path in the template.** cosmic-app-template still installs metainfo into `share/appdata`. `share/metainfo` is correct and is what cosmic-ext-camera, Locket and Peek use.

---

## Identity: one reverse-DNS id, everything named after it

*Seen in: all eight repos · grabit follows · Locket follows · Peek follows*

The RDNN `APP_ID` is the primary key of a COSMIC application. Every project declares it once as a const and refers to that. It is simultaneously:

- the `cosmic::Application::APP_ID` constant,
- the cosmic-config store — `~/.config/cosmic/<APP_ID>/v<N>/<key>`,
- the desktop entry filename — `<APP_ID>.desktop` — and its `Icon=` and `StartupWMClass=` values,
- the metainfo filename and its `<id>`, joined to the desktop entry by `<launchable type="desktop-id">`,
- the icon name — `share/icons/hicolor/…/apps/<APP_ID>.svg`,
- the D-Bus name for single-instance activation, with the object path derived by replacing `.` with `/` and prefixing a slash.

```rust
const APP_ID: &'static str = "com.system76.CosmicLauncher";
// → ~/.config/cosmic/com.system76.CosmicLauncher/v1/
// → com.system76.CosmicLauncher.desktop / .metainfo.xml
// → Icon=com.system76.CosmicLauncher
```

First-party apps use `com.system76.Cosmic<Name>` with a `cosmic-<name>` binary. Community apps use a domain they control or `io.github.<user>.<App>` — cosmic-ext-camera is `io.github.cosmic_utils.camera` — with an unprefixed binary. grabit uses `io.github.idominikos.Grabit` throughout, even though it is a daemon whose desktop entry is `NoDisplay=true` and exists only to autostart it.

> **⚠ Renaming is a data migration.** The config store, the XDG data directory, the D-Bus name and every filename move together, and nothing migrates them for you. A rename silently starts the app with empty settings and an empty index. Pick the namespace the rest of your suite uses, the first time.

### One icon per size, or one scalable

> **⚡ Two readings, both true.** The general survey found first-party apps shipping one SVG per size under `hicolor/<size>/apps/`, so small sizes can be drawn on the pixel grid rather than scaled down from a detailed drawing. Locket, Peek and grabit — and the template's install recipe — install a single `scalable/apps/<APP_ID>.svg`. Both resolve; the per-size set is the higher-effort, better-looking option once the app is on a panel.

---

## Generated XDG files: xdgen

*Seen in: cosmic-app-template, cosmic-applet-template · grabit adopted (with a fix) · Locket deferred · Peek rejected*

Both templates now generate the desktop entry and metainfo at build time from the same Fluent catalogue the UI uses, because the name, comment and keywords are translatable and would otherwise be three copies to keep in sync. Three reserved keys carry it: `app-title`, `app-comment`, `app-keywords`. Adding a language then means adding an `.ftl` file and nothing else.

```rust
// build.rs
use xdgen::{App, Context, FluentString};

let ctx = Context::new("i18n", env::var("CARGO_PKG_NAME").unwrap()).unwrap();
let app = App::new(FluentString("app-title"))
    .comment(FluentString("app-comment"))
    .keywords(FluentString("app-keywords"));

fs::write(output.join("app.desktop"),
          app.expand_desktop("resources/app.desktop", &ctx)?)?;
fs::write(output.join("app.metainfo.xml"),
          app.expand_metainfo("resources/app.metainfo.xml", &ctx)?)?;
```

Output carries one variant per language in the catalogue:

```ini
Name=grabit
Name[en]=grabit
Comment=Selection-triggered actions for Wayland desktops
Comment[en]=Selection-triggered actions for Wayland desktops
```

> **⚠ Two things nobody documents.**
> 1. **xdgen replaces keys; it does not add them.** A template with no `Name=` line gets no `Name=` line, silently. The metainfo template likewise needs `<name>` and `<summary>` present or the expansion fails outright with *missing tag name*. Put placeholder values in and let xdgen localise them.
> 2. **The templates hardcode `target/xdgen/`,** which breaks under `CARGO_TARGET_DIR`. Reading the variable in build.rs costs one line, and without it `just install` cannot find the generated files.

> **⚡ Adoption is unsettled — and the sources disagree about it.** The general survey records cosmic-app-library adopting xdgen at `385db1e` on 2026-08-19. The Peek notes, reading the same revision, say cosmic-launcher, cosmic-app-library and cosmic-edit all still check static metadata into `data/` or `res/`. One of those readings is wrong; check the revision yourself before treating the shipping apps as having moved. What is certain: both templates generate, hand-written static files still work and are still common.

The three projects split three ways. grabit adopted it with the `CARGO_TARGET_DIR` fix. Locket defers it until a second language actually exists. Peek tried it and found that adding xdgen re-resolved the dependency graph in a way that dropped zbus's `blocking` feature and broke libcosmic's single-instance support — so Peek follows the shipping apps rather than the templates here.

---

## Desktop entry & AppStream

*Seen in: cosmic-edit, cosmic-ext-camera, cosmic-app-template · measured on Peek · Locket follows · Peek follows*

The desktop entry is unremarkable except that `Icon` and `StartupWMClass` are both the APP_ID. The metainfo is where applications quietly diverge, and where a software centre notices.

```ini
Icon=io.github.example.App
StartupNotify=true
StartupWMClass=io.github.example.App
```

The full metainfo shape, as cosmic-edit and cosmic-ext-camera write it:

```xml
<component type="desktop-application">
  <id>io.github.example.App</id>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>GPL-3.0-or-later</project_license>
  <name/> <summary/> <description/>
  <launchable type="desktop-id">io.github.example.App.desktop</launchable>
  <provides>
    <id>com.system76.CosmicApplication</id>   <!-- how a COSMIC app says so -->
    <binary>app</binary>
  </provides>
  <requires><display_length compare="ge">360</display_length></requires>
  <supports><control>keyboard</control><control>pointing</control></supports>
  <categories/> <keywords/>
  <url type="homepage"/> <url type="bugtracker"/> <url type="vcs-browser"/>
  <developer id="io.github.example"><name>…</name></developer>
  <content_rating type="oars-1.1"/>
  <branding><color type="primary" scheme_preference="light">#…</color></branding>
  <screenshots/>
  <releases><release version="1.0.0" date="…"/></releases>
</component>
```

- `<provides><id>com.system76.CosmicApplication</id>` is the marker the COSMIC store and app library filter on.
- The `<release>` version must match `version` in Cargo.toml — the About page reads the crate's version, and two numbers that disagree make a bug report unanswerable.
- On a rename, `<provides><id>old.id</id>` and `<replaces>` keep a published component's history and reviews attached.

### Validate both in CI; nothing else checks them

`desktop-file-validate res/*.desktop` and `appstreamcli validate --no-net …` catch what the compiler never sees. Use `--no-net` on pull requests — a fork cannot reach URLs that only exist after merge — and run the networked pass separately; that is what catches a dead repository URL before the first packager does.

> **⚠ No line continuation in the Desktop Entry spec — measured.** A trailing backslash is part of the value, so a `MimeType=` list wrapped across lines for readability parses as one invalid type and registers nothing. Peek's desktop entry had a wrapped 80-entry MIME list; `update-desktop-database` registered zero of them, so "Open With → Peek" — one of its three documented entry points — had never worked. Collapsing the list onto one line registered all 80.

---

## Localisation

*Seen in: all four apps, both templates, libcosmic · grabit scoped down · Locket follows · Peek follows*

The most rigidly shared convention in the ecosystem. `i18n-embed` with the Fluent backend, catalogues compiled in through `rust-embed`, and a crate-local `fl!` macro that is byte-identical across cosmic-launcher, cosmic-app-library and cosmic-edit. The file is copied, not depended on. Every COSMIC application is translatable, applets included.

```toml
i18n-embed    = { version = "0.16", features = ["fluent-system", "desktop-requester"] }
i18n-embed-fl = "0.10"
rust-embed    = "8"
```

```rust
// src/i18n.rs (localize.rs in shipped apps) — verbatim from the template
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = /* … */;

#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id)
    }};
    ($message_id:literal, $($args:expr),*) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

// main.rs, before any widget exists
i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());
```

The loader is initialised in `main` before the application is constructed, because `fl!` is evaluated during `init` when nav-bar entries and the About page are built. Strings live in `i18n/<locale>/<crate_name>.ftl`; the layout is what Hosted Weblate expects, which is why cosmic-launcher ships 74 languages without anyone hand-editing them — translation commits arrive as pull requests against `i18n/`. Do not invent the layout locally.

### Six things worth knowing before the first catalogue

1. **The file is named after the crate's package name** — the underscored crate name, not the binary: `locket-cosmic.ftl` for the crate `locket-cosmic`. Get it wrong and the build fails, saying which filename it wanted; that error is the fastest way to find out.
2. **`fl!()` is compile-time checked against `i18n/en`.** A typo in an id is a build error, not a label reading `some-id` at runtime. This is the single biggest reason to use it rather than a HashMap.
3. **Fluent wraps every interpolated value in bidi isolation marks** (U+2068, U+2069) so a number keeps its direction inside an RTL sentence. Invisible in a text widget; not invisible to `str::contains`, so tests asserting on formatted output must strip them — and they appear as stray characters in a terminal. `loader.set_use_isolating(false)` turns them off, and it must be called *after* `select()`, which rebuilds the bundles and discards the setting.
4. **Plurals go through Fluent, not through Rust.** Pass the count as a number and let the catalogue select. Handing it over as a string silently produces "1 minutes ago" because there is nothing left for the plural rules to select on, and the rules differ per language in ways an `if count == 1` cannot express. `format!("{}s", singular)` is English-only.
5. **Dropdown labels have to outlive the view.** `widget::dropdown` borrows its label slice for the lifetime of the element it returns, so a `Vec<String>` built inside `view()` will not compile. A `LazyLock<Vec<String>>` resolved after `i18n::init` is the idiom that works.
6. **Do not translate stored data or logs.** Slot labels, collection names, anything written to a file or served over D-Bus stays in one language; translating it rewrites what is on disk. Keep English labels in the core crate for the CLI and map to fluent ids in the GUI. Log lines stay untranslated on purpose — a translated log line is harder to grep for, not easier.

```fluent
line-count = { $count ->
        [one] { $count } line
       *[other] { $count } lines
    }
```

grabit's catalogue holds only the application identity, one failure notification and `grabit doctor`: every label and tooltip in its popup comes from the user's own action manifests, so it is their text and not grabit's to translate.

---

## The `cosmic::Application` shape

*Seen in: all four apps, both templates · grabit follows · Locket follows · Peek follows*

```rust
impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;   // SingleThreadExecutor for applets; launcher uses executor::single
    type Flags = Flags;
    type Message = Message;
    const APP_ID: &'static str = "io.github.example.App";

    fn core(&self) -> &Core;
    fn core_mut(&mut self) -> &mut Core;
    fn init(core: Core, flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>);
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>>;
    fn view(&self) -> Element<'_, Self::Message>;
    fn view_window(&self, id: SurfaceId) -> Element<'_, Self::Message>;   // multi-surface apps
    fn subscription(&self) -> Subscription<Self::Message>;
}
```

- **`core` is field zero** and is never touched except through `core()` / `core_mut()`. It carries the theme, window state, applet context and scale factor.
- **`Flags`** is how anything the application needs but cannot rebuild gets in — channels, parsed arguments, handles.
- `use cosmic::prelude::*` brings in `Apply`, `Element` and `Task`. The `.apply(container)` idiom for wrapping a widget comes from `Apply` and is used constantly.
- **`Message` is a flat enum**, `#[derive(Debug, Clone)]`, one variant per user action or async completion. Async work is a `Task` returning a message, never a blocking call inside `update`.
- **Entry points differ by kind:** `cosmic::app::run` for a window, `cosmic::applet::run` for a panel applet, `cosmic::app::run_single_instance` when a second invocation should reach the running process over D-Bus.
- The `About` widget expects `env!("CARGO_PKG_VERSION")`, `CARGO_PKG_REPOSITORY` and `CARGO_PKG_LICENSE` — another reason to keep Cargo.toml metadata complete.

### Hooks the trait already provides

Worth reading before writing an event listener: several exist precisely to stop you doing it by hand. Implement them and COSMIC's own layout appears; there is no separate window shell to build.

| Hook | When |
|---|---|
| `on_escape` | Escape pressed |
| `on_search` | Ctrl+F, from libcosmic's own keyboard navigation |
| `on_nav_select`, `on_nav_context` | nav bar interaction |
| `on_app_exit` | before the application closes |
| `on_close_requested(id)` | a window, or an applet popup, wants to close |
| `on_context_drawer` | the context drawer was toggled |
| `dbus_activation` | a second launch handed off to this instance (`Activate`, `ActivateAction`, `Open`) |
| `system_theme_update`, `system_theme_mode_update` | the desktop theme or light/dark mode changed |
| `header_start`, `header_end`, `nav_model`, `context_drawer`, `dialog` | the standard chrome |

`core.keyboard_nav` is on by default, so Tab / Shift-Tab, Escape, F11 and Ctrl+F are handled for you. Listening for Ctrl+F yourself means handling it twice; layer-shell overlays that own their keyboard handling turn it off (see Layer-shell surfaces).

### Structural pieces of a windowed app

- `nav_bar::Model`, with `.data::<Page>()` attaching the page to each entry, and `nav_model()` returning it — or `None` to hide the sidebar.
- `header_start()` → `menu::bar`; `header_end()` → action buttons.
- `context_drawer()` → `context_drawer::about(&self.about, …)`, keyed off a `ContextPage` enum and `core.window.show_context`.
- `dialog()` → `widget::dialog()` with `primary_action` / `secondary_action`.
- `widget::toaster(&self.toasts, content)` wrapping the whole view.
- `update_title()`, called from `init` and `on_nav_select`, setting both the window title and the header title.
- `widget::settings::section().title(…).add(widget::settings::item(…))` for preference rows.

### Shortcuts belong in a KeyBind table

```rust
key_binds: HashMap<menu::KeyBind, MenuAction>,
```

> **⚠ Hand-rolled matching is layout-broken.** `KeyBind::matches(modifiers, &key, Some(&physical))` compares the whole modifier set — so Ctrl+Shift+N does not fire a Ctrl+N binding — and falls back to the physical key position, which is the only reason Ctrl+N works on a Greek or Cyrillic layout. Matching `Key::Character("n")` by hand silently does nothing on those layouts.

The same table is what `menu::items` reads to print the shortcut beside each menu entry — which is how anybody discovers a shortcut exists at all.

### Single instance means D-Bus activation, not a lock file

*Seen in: cosmic-launcher, cosmic-app-library · Peek follows · Locket follows*

`run_single_instance` claims the app ID on the session bus. A second invocation is delivered to the running process as an activation, so the command *is* the way to talk to the daemon — which is what makes it bindable to a shortcut. Arguments travel through a `CosmicFlags` impl and arrive in `dbus_activation`.

### Daemons and other non-windowed applications

*Seen in: cosmic-launcher, cosmic-app-library · grabit follows · Peek partial*

```rust
Settings::default()
    .antialiasing(true)
    .client_decorations(true)
    .debug(false)
    .default_text_size(16.0)
    .scale_factor(1.0)
    .no_main_window(true)     // surfaces are created on demand
    .exit_on_close(false)     // closing one is not the app exiting
```

Both overlays build settings with this identical seven-call chain. It is copied rather than derived, so matching it is the cheapest way to behave like the rest of the desktop. With `no_main_window`, `view()` is unreachable — cosmic-launcher writes `unreachable!("No main window")` — and `view_window(id)` dispatches on surface id. Peek matches `antialiasing`, `no_main_window` and `exit_on_close` and diverges on the rest for documented reasons: `transparent(true)` because the overlay covers the output and would otherwise composite an opaque black rectangle, `client_decorations(false)` because there is no toplevel to decorate, and `is_daemon(true)` to stay resident.

**Subscriptions take function pointers.** `Subscription::run_with(data, builder)` takes a plain `fn` pointer, not a closure, and identifies the subscription by hashing `data`. Everything the stream needs must travel inside `data`, and `data` must implement `Hash`. For channels — which cannot be meaningfully hashed and never change — hash a constant name and clone the channel out inside the builder. `listen_raw(|event, status, id| …)` is how surface-tagged input arrives; the `id` is what distinguishes one surface's events from another's.

---

## Configuration: cosmic-config

*Seen in: all four apps, both templates · Locket follows · Peek follows · grabit rejected for actions*

One struct, derived, versioned. Everything stores configuration the same way, and nothing polls for changes.

```rust
use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Default, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    demo: String,
}

// in subscription()
self.core()
    .watch_config::<Config>(Self::APP_ID)
    .map(|update| Message::UpdateConfig(update.config))
```

What that actually does on disk:

- **Each field is a separate RON file** — `~/.config/cosmic/<APP_ID>/v1/<field>`, named after the field. The derive generates a per-field read, so a missing field falls back to `Default` independently of the others. Renaming a field is therefore a migration, and renaming the app id moves the whole store.
- **`#[version = N]` is a directory, not a schema check.** Version 2 reads `v2/`, falling back to `v1/` through a "previous" chain for keys it does not find.
- **Loading returns partial success.** `Config::get_entry` gives `Result<Self, (Vec<Error>, Self)>` — the errors and a usable value. Every application logs the errors and carries on with the defaults, so a corrupt key costs one setting rather than the application.
- **Live reload is the norm, not a feature.** The watcher already filters to the keys the struct owns and only emits when a value actually changed, so a hand-written watcher, key list and change filter are all redundant. With the `dbus-config` feature it goes through cosmic-settings-daemon rather than polling inotify, so edits made in COSMIC Settings apply live. For a resident process that is the difference between a setting taking effect now and at next login.

> **⚠ Two sharp edges.**
> 1. **The version chain resolves new → old only.** An application pinned to v1 running against a desktop that writes v2 silently reads stale values, with no error at all.
> 2. **`Error::is_err()` does not filter what you expect.** It exists to exclude `NoConfigDirectory` and `NotFound` from logging, but a key absent from the system defaults arrives as `GetKey(io::ErrorKind::NotFound)`, which that filter does not exclude — so a libcosmic newer than the installed COSMIC logs spurious ERRORs for theme keys the desktop has not shipped yet.

**grabit's rejection:** its configuration is one TOML file per action, and being able to drop an action in, edit it and delete it is the feature — the same thing a PopClip extension is. Putting that behind a settings daemon would trade the point of the design for ecosystem tidiness. The daemon's own settings could reasonably move to cosmic-config later; the actions should not.

---

## Theme & spacing

*Seen in: cosmic-launcher, cosmic-app-library, libcosmic · grabit partial · Locket follows · Peek follows*

Never write a raw pixel value for spacing or a corner radius, and never a literal colour. The theme answers the questions an app might be tempted to add settings for.

```rust
let cosmic = theme.cosmic();
let padding: Padding = [cosmic.space_xxs(), cosmic.space_m()].into();
let radius  = cosmic.corner_radii.radius_s;
let space_s = cosmic::theme::spacing().space_s;            // when there is no theme in hand
let err     = cosmic::theme::active().cosmic().destructive_color();

widget::container(content).class(theme::Container::Dropdown)   // floating surface
widget::button::custom(content).class(theme::Button::Icon)
```

- **Semantic classes** handle light, dark and accent changes for you. `Container` covers WindowBackground, Background, Card, Dialog, Dropdown, List, Primary, Secondary, Tooltip, Transparent (the default) and Custom. `Button` covers Standard, Suggested, Destructive, Icon, IconVertical, Link, Text, the applet and menu variants, and Custom.
- **Colours** come from the semantic accessors — `bg_color()`, `on_bg_color()`, `accent_color()`, `background(is_transparent).base`, `bg_divider()`. Styling outside the standard classes means a `theme::X::Custom` closure that reads the theme and returns a `Style`; both overlays do this at length for buttons.
- A resident app answers `system_theme_update` and `system_theme_mode_update` so it repaints when the desktop changes mode.

grabit uses `Container::Dropdown` for the bar and `Button::Icon` for the actions; its padding is still literal and should move to spacing tokens.

### The frosted-glass switches

cosmic-theme carries a set of booleans the whole desktop obeys. An application that requests compositor blur is expected to gate it on the key matching its kind. Ignoring these means a user who turned frosting off desktop-wide still gets a blurred surface from you — an app-level blur toggle is a setting the desktop already has.

| Key | Governs |
|---|---|
| `frosted` | blur strength |
| `frosted_windows` | ordinary application windows |
| `frosted_system_interface` | shell surfaces — launcher, OSD, layer surfaces |
| `frosted_panel` | the panel |
| `frosted_applets` | applet popups |
| `frosted_maximized_apps` | windows while maximised |

```rust
if self.core.system_theme().cosmic().frosted_system_interface {
    // … send a blur region
}
```

libcosmic's `Core::frosted(&theme)` picks the right key from `Core::app_type()` (Window / System / Applet); cosmic-launcher reads `frosted_system_interface` directly.

---

## Icons

*Seen in: cosmic-launcher, cosmic-app-library, libcosmic · Locket follows*

### Check the name resolves along COSMIC → Pop → hicolor

The COSMIC icon theme inherits Pop, then hicolor (`index.theme: Inherits=Pop,hicolor`). A symbolic name that exists in Adwaita and nowhere else resolves to nothing on a COSMIC session — the widget renders blank, with no warning.

```sh
find /usr/share/icons/{Cosmic,Pop,hicolor} -name 'the-name-symbolic.svg'
```

`credit-card-symbolic` is the trap Locket already fell into; the name COSMIC and Pop both ship is `payment-card-symbolic`.

### Give lookups an explicit fallback chain

The default fallback truncates at each `-`, so `com.example.App` degrades to whatever `com` happens to be:

```rust
icon::from_name(name)
    .prefer_svg(true)
    .size(64)                            // resolve larger than you draw
    .fallback(Some(IconFallback::Names(vec![
        "application-default".into(),
        "application-x-executable".into(),
    ])))
    .handle()
```

### Where to resolve handles

> **⚡ The sources disagree on cost.** The general survey says `icon::from_name(..).handle()` performs a freedesktop icon-theme lookup that walks the search path and retries against shorter prefixes on a miss, and that both overlays keep handle vectors (`launcher_item_icon_handles`, `entry_icon_handles`) rebuilt in `update` and cloned during drawing — so resolve in update, never in view. The Peek notes say `from_name` goes through `freedesktop_icons::lookup(…).with_cache()`, so calling it in view is fine. Both are consistent with the code: the lookup is cached, and the shipped apps still cache handles anyway. Caching in state is the safe default for lists that redraw often; the fallback chain matters more than either.

- **Symbolic icons** (`-symbolic` suffix) are recoloured by the panel for the active theme. An applet's panel button should use one; a full-colour icon there will look wrong next to every other applet.
- **Embed the application's own icon for the About page** — `include_bytes!` plus `widget::icon::from_svg_bytes` — because `icon::from_name(APP_ID)` finds nothing in a build that has not been installed yet, and a blank About icon is the first thing anyone running `cargo run` sees.
- **Ellipsize text that comes from outside the application** — desktop entry names, file paths — because iced wraps by default: `text::body(line).ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))`.

---

## Panel applets

*Seen in: cosmic-applet-template, cosmic-panel, libcosmic · Locket follows*

An applet is a normal `cosmic::Application` started with `cosmic::applet::run`, plus one extra concept: `self.core.applet`.

```rust
fn main() -> cosmic::iced::Result {
    i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());
    cosmic::applet::run::<Applet>(())
}
```

- `type Executor = cosmic::SingleThreadExecutor` — an applet is one small process, and the panel starts one per applet.
- `fn style() -> Option<Style> { Some(cosmic::applet::style()) }`.
- `view()` draws only the panel button (`core.applet.icon_button(name)`); `view_window(id)` draws the popup, wrapped in `core.applet.popup_container(…)`.
- `on_close_requested(id)` → a `PopupClosed(id)` message. The popup is a real window; the compositor can close it without asking.

The popup lifecycle is stereotyped:

```rust
Message::TogglePopup => {
    return if let Some(p) = self.popup.take() {
        destroy_popup(p)
    } else {
        let new_id = Id::unique();
        self.popup.replace(new_id);
        let mut settings = self.core.applet.get_popup_settings(
            self.core.main_window_id().unwrap(), new_id, None, None, None);
        settings.positioner.size_limits = Limits::NONE
            .max_width(372.0).min_width(300.0)
            .min_height(200.0).max_height(1080.0);
        get_popup(settings)
    };
}
Message::PopupClosed(id) => {
    if self.popup.as_ref() == Some(&id) { self.popup = None; }
}
```

- `get_popup_settings` already carries sane size limits (min 360 wide) and the right anchor for the panel's edge.
- Two ways to open a popup: `get_popup` / `destroy_popup` (the template), or the newer surface actions `app_popup::<App>(…)` used by the shipped applets — which is what supports hover-to-open.
- The applet helper carries everything that has to match the panel's geometry and configuration: `icon_button`, `text_button`, `popup_container`, `suggested_size`, `suggested_padding`, `is_horizontal`, `autosize_window`. Use them rather than measuring the panel yourself — it can be any size and either orientation.
- `cosmic::applet::menu_button` and `menu_control_padding` are the standard popup menu row and its padding. cosmic-launcher redefines both locally; that is duplication, not a second convention.
- **To launch the full application from the panel:** ask for an XDG activation token (`activation_token_subscription`, the `applet-token` feature), then `cosmic::desktop::spawn_desktop_exec(exec, env, Some(app_id), false)` with `desktop-systemd-scope`. Without a token, the new window comes up unfocused behind the panel; without the scope, restarting the panel takes the window down with it.

### The applet's desktop entry is its configuration

```ini
[Desktop Entry]
Type=Application
Name=App Indicator
Exec=app-applet
Icon=io.github.example.App
Terminal=false
Categories=COSMIC;
NoDisplay=true
X-CosmicApplet=true
X-CosmicHoverPopup=Auto
```

Others cosmic-panel parses: `X-OverflowPriority`, `X-OverflowMinSize`, `X-CosmicShrinkable` (behaviour when the panel runs out of room), `X-MinimizeApplet`, `X-NotificationsApplet`, `X-HostWaylandDisplay`.

> **⚠ Shared blast radius.** cosmic-panel hosts applets as a nested compositor. A fatal Wayland protocol error in that process takes down the panel, the dock and every applet at once. Nothing an applet does in Rust can protect against a compositor-side protocol violation — which is an argument for keeping applet surface handling boring.

---

## Layer-shell surfaces

*Seen in: cosmic-launcher, cosmic-app-library, cosmic-panel, libcosmic · measured on cosmic-comp · grabit follows · Peek follows*

The most valuable material in these repositories, and the part with the least documentation anywhere else. For shell-like surfaces — launcher overlays, OSDs, docks, action bars — these are the settings that recur. A composite of what the shipped surfaces set, not a literal copy of one:

```rust
SctkLayerSurfaceSettings {
    id: SurfaceId::unique(),
    layer: Layer::Overlay,                         // above the panel — must be explicit
    keyboard_interactivity: KeyboardInteractivity::Exclusive,   // or ::None for pointer-only
    input_zone: None,                              // None = all input; Some(vec![]) = click-through
    anchor: Anchor::TOP | Anchor::LEFT,
    output: IcedOutput::Active,
    namespace: "my-surface".into(),
    margin: IcedMargin { top: y, left: x, ..Default::default() },
    size: None,
    exclusive_zone: -1,
    size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
}
```

- **`Layer::Overlay` has to be set explicitly.** `SctkLayerSurfaceSettings` defaults to `Layer::Top`, which the panel also occupies.
- **`exclusive_zone: -1`** opts out of other clients' exclusive zones, so the surface covers the whole output including under panels, and stops the panel and dock reserving space against you. The default is 0, which is not the same thing. Any full-screen overlay wants this.
- **`keyboard_interactivity`** — `Exclusive` is required to receive typing; without it focus stays on the previously active window. A pointer-only overlay (grabit's hunter) uses `None` so it never takes keyboard focus.
- **`input_zone`** — `None` accepts all input, `Some(vec![])` accepts none. It compiles to plain `wl_surface.set_input_region`, so it is portable to any wlr-layer-shell compositor, and libcosmic records the surface in a `to_commit` map so the region actually reaches the wire. Doing this by hand through a toolkit that does not track pending commits is where the bodies are buried: a region set on a surface that renders no new frame is silently never applied.
- **`margin`** is how you position an anchored surface. Anchor to two adjacent edges and the margins become coordinates. There is no positioner, so flipping and clamping near screen edges is the client's job.
- **`size: None`** autosizes to content; `Some((None, None))` with opposite-edge anchors stretches across the output.
- **`IcedOutput::Active`** targets the focused output, removing any need to build one surface per monitor.
- **`namespace`** is a string the compositor can key rules off.

### Declare `AppType::System` and turn off keyboard nav

*Seen in: cosmic-launcher, cosmic-app-library · Peek follows*

```rust
fn init(mut core: Core, flags: Flags) -> (Self, Task<Message>) {
    core.set_app_type(cosmic::core::AppType::System);
    core.set_keyboard_nav(false);
    // …
```

Both overlays open `init` the same way. `AppType::System` switches libcosmic's frosted and corner-radius decisions from the window settings to the system-interface settings, which is what governs the panel and the launcher. Disabling keyboard navigation stops libcosmic's own subscription interpreting Tab, Escape, F11 and Ctrl+F in parallel with the app's own handler.

### Create and destroy — never hide and show

`get_layer_surface` per appearance, `destroy_layer_surface` per disappearance. This is not a stylistic preference. Hiding and re-showing can make a toolkit build a second `zwlr_layer_surface_v1` on the same `wl_surface`, which smithay-based compositors reject as a role reassignment and answer by dropping the client. A fresh surface each time gives a fresh `wl_surface` and sidesteps it entirely.

### The dummy surface, for two reasons

cosmic-launcher keeps a permanently mapped throwaway surface on `Layer::Bottom` with an empty input region. It does two jobs:

- **The process always owns one surface.** A toolkit that loses its last surface tends to close its Wayland connection, and the next surface then has nothing to be created on. Any application whose surfaces all come and go needs one.
- **It starts receiving `overlap_notify` events before the visible surface maps**, so the real one appears already positioned. `overlap_notify` + `set_padding` is how cosmic-launcher avoids covering the panel instead of hardcoding an offset.

### Popups get a positioner; layer surfaces do not

For a menu at a point, an xdg popup parented to a layer surface does the constraint solving compositor-side — `anchor_rect`, `gravity`, and `constraint_adjustment: 15` to flip and slide on both axes. libcosmic's applet tooltips make a popup entirely click-through with an off-screen zero-size `input_zone`.

### Blur

`commands::blur::blur(id, regions)` requests compositor backdrop blur via `ext-background-effect-v1`. The region is double-buffered surface-local state: it only takes effect against a committed buffer, and must be resent whenever the geometry changes. Gate it on the frosted key for your surface kind (see Theme).

### A newer surface API exists, and the launcher has moved to it

*Seen in: cosmic-launcher · Peek stays on the raw call*

cosmic-launcher no longer calls `get_layer_surface` directly. It goes through `cosmic::surface::surface_task(app_layer_shell(…))` with a `LiveSettings { padding, corners, blur }` closure, so padding, compositor-side corner rounding and blur are part of surface creation rather than three follow-up commands.

Not always the right move. `LiveSettings.blur` is a boolean — the whole surface or none of it. An overlay that needs a blur region shaped to something smaller than its surface still wants the raw call, which is why Peek stays on it.

---

## The pointer problem

*Seen in: cosmic-launcher · measured on cosmic-comp with `WAYLAND_DEBUG=1` · grabit designed around it*

Worth stating plainly, because it is the thing nothing in the ecosystem solves.

**No COSMIC project can ask where the pointer is.** cosmic-launcher tracks `Mouse(CursorMoved)` within its own surface and only ever uses the value once the user has already interacted with it. There is no protocol for a global cursor position, and `wp_pointer_warp_v1` does not help, because warping requires the surface to already hold pointer focus.

> **Measured, not inferred.** A compositor re-evaluates which surface the pointer is over when the pointer *moves*, not when a surface appears beneath it. On cosmic-comp, a full-screen overlay accepting all input received no pointer event whatsoever until the mouse was nudged.
>
> The same property is also a safety net. Because focus does not transfer until the pointer moves, an overlay that opens its input region does not steal the next click — a click made without moving still reaches the application underneath.

grabit is built on three surfaces, with the hunter torn down after four seconds if nothing moves:

| Surface | Role |
|---|---|
| **dummy** | One pixel, background layer, empty input region. Never destroyed — it exists so the process always owns a surface. |
| **hunter** | Full-screen, transparent, accepts all pointer input. Created when a selection settles; destroyed the moment it learns where the pointer is. |
| **bar** | The buttons, anchored top-left with margins set to the pointer position. Its input region is its own bounds. |

---

## Launching applications

*Seen in: cosmic-launcher, cosmic-app-library, libcosmic · measured: systemd scope, token in child env · Peek follows · Locket follows (applet)*

Launching another program is not `Command::spawn`. This is the most consistently implemented and most easily missed sequence in the ecosystem; both cosmic-launcher and cosmic-app-library do all three steps.

**1 · Ask the compositor for an activation token, bound to your own surface**

```rust
request_token(Some(String::from(Self::APP_ID)), Some(self.window_id))
    .map(|token| Message::ActivationToken(token, app_id, exec, gpu, terminal))
```

**2 · Pass it to the child under both names**

```rust
env.push(("XDG_ACTIVATION_TOKEN".into(), token.clone()));
env.push(("DESKTOP_STARTUP_ID".into(),   token));   // XWayland / startup notification
```

Without this the compositor sees a window appear from a process it has no reason to believe the user asked for, and may apply focus-stealing prevention — the new window comes up unfocused, behind whatever launched it.

**3 · Resolve the GPU environment from switcheroo-control, never hardcode it**

```rust
let proxy = SwitcherooControlProxy::new(&system_bus).await?;
if !proxy.has_dual_gpu().await? { return None; }
let gpu = match preference {
    GpuPreference::Default        => gpus.into_iter().find(|g| g.default),
    GpuPreference::NonDefault     => gpus.into_iter().find(|g| !g.default),
    GpuPreference::SpecificIdx(i) => gpus.into_iter().nth(i as usize),
};
env.extend(gpu.environment);        // vendor-specific; not always PRIME
```

The crate is `switcheroo-control` from pop-os/dbus-settings-bindings, the same repository libcosmic already pulls cosmic-settings-daemon from. Hardcoding `__NV_PRIME_RENDER_OFFLOAD` does nothing on an AMD or Intel hybrid, and is actively wrong on a single-GPU machine.

**Then hand off to libcosmic:**

```rust
cosmic::desktop::spawn_desktop_exec(exec, env, Some(&app_id), terminal).await;
```

This expands `Exec` field codes, resolves the user's configured terminal for `Terminal=true` entries, and — with the `desktop-systemd-scope` feature — asks systemd to put the new PID in its own transient scope named `app-cosmic-<app-id>-<pid>.scope`, following systemd's DESKTOP_ENVIRONMENTS guidance. Underneath, `cosmic::process::spawn` double-forks with `setsid` and reaps the intermediate child, so a long-lived process does not accumulate zombies. The launched application then belongs to the session rather than to whatever started it: it neither dies with the launcher nor inherits its cgroup.

The feature pulls in `desktop`, which pulls in `process`. Enabling it without calling the function is a cost with no benefit.

> **Measured on a live session.** A launch through this path lands as `app-cosmic-org.gnome.FileRoller-296830.scope`, with both `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` present in the child's `/proc/<pid>/environ`, carrying the same token value.

### Desktop actions

A desktop entry may be matched by one of its actions ("New Window", "New Private Window"). The action's own `Exec` wins over the entry's, and `DesktopAction.name` is the localised display name, not the group id:

```rust
let exec = match action_name {
    Some(name) => entry.desktop_actions.into_iter()
        .find(|a| a.name == name).map(|a| a.exec),
    None => entry.exec,
};
```

### If you are a pop-launcher client

*Measured on pop-launcher 1.2.7, driven directly over its stdio protocol.* Two behaviours are documented nowhere.

> **⚠ pop-launcher does not start applications.** `Activate` on a desktop-entry result makes it reply `DesktopEntry { path, gpu_preference, action_name }` and leave the launching to you — which is what makes steps 1–3 possible at all. A client that ignores that response silently launches nothing.

> **⚠ Close cancels an activation in flight.** Writing `Activate` and `Close` back to back loses the `DesktopEntry` reply entirely; with a delay between them it arrives. Do not tell the service the interaction ended while you are still waiting on it. cosmic-launcher sidesteps this by hiding only after the launch completes.
>
> | Sequence | Replies |
> |---|---|
> | `activate` only | 1 |
> | `activate` + `close` | 0 |
> | `activate`, 0.3 s, `close` | 1 |

---

## Shared components worth reaching for, and gaps in libcosmic

*Seen in: cosmic-edit, cosmic-ext-camera, cosmic-app-library, libcosmic*

Things the ecosystem has already solved, that are easy to rebuild by accident — and the things it has not solved, so you do not go looking.

| Instead of | Use |
|---|---|
| syntect's bundled syntaxes and themes | `two-face` for ~250 languages, and `cosmic-syntax-theme`'s `COSMIC_DARK_TM_THEME` / `COSMIC_LIGHT_TM_THEME`, with `settings.background` and `.gutter` zeroed so your own surface shows through |
| caching icon lookups yourself | `cosmic::widget::icon::from_name` — it goes through `freedesktop_icons::lookup(…).with_cache()` (though the shipped apps still keep handle vectors; see Icons) |
| hand-rolled easing and a frame clock | `cosmic::iced::animation::Animation` (lilt-backed, interruptible). `cosmic::anim` also exists but is a simpler lerp/smootherstep helper |
| an `image::Handle` per video frame | an iced_wgpu primitive with persistent textures — what cosmic-ext-camera does. Failing that, hand the decoded buffer to `Handle::from_rgba` by value; `Bytes::from(Vec<u8>)` does not copy |
| a hand-written config watcher | `core().watch_config::<Config>(APP_ID)` — already filters to your keys and only emits on change |
| matching key characters by hand | a `HashMap<menu::KeyBind, MenuAction>`, which also prints the shortcut beside the menu entry |
| measuring the panel yourself | `core.applet.suggested_size()` / `suggested_padding()` / `is_horizontal()` |

### Gaps in libcosmic worth knowing about

- **There is no grid widget.** cosmic-app-library's application grid is `.chunks(7)` into rows, with the comment `// TODO grid widget in libcosmic` still in place. Hand-rolling a grid is the current answer, not a workaround.
- **There is no generic opacity wrapper.** Only `image` and `svg` expose one, so fading a composite of widgets means threading an alpha through every colour you produce.
- **`iced::widget::Float` only scales above 1.0** — the transform is gated on `self.scale > 1.0`, so a 0.96 → 1.0 entrance animation renders at full size throughout.
- **Scrolling to a focused item is approximate.** cosmic-launcher computes a proportional `RelativeOffset` with the comment "ideally we could use an operation to scroll exactly to a specific widget".
- **No global pointer position** — see The pointer problem.
- **`LiveSettings.blur` is all-or-nothing** — see Layer-shell surfaces.

---

## Where the ecosystem does *not* agree

Worth knowing so you do not go looking for a standard that is not there.

- **Logging.** cosmic-launcher uses `tracing` with a journald layer and an `EnvFilter` defaulting to `warn,<crate>=debug`; cosmic-ext-camera uses `tracing` with `fmt`; cosmic-app-library uses `pretty_env_logger`; cosmic-edit uses `env_logger`. There is no convention. `tracing-journald` first with `tracing_subscriber::fmt` as the fallback is the most capable and the one the newest code uses — a desktop app is usually started by an activation or a desktop entry, and in both cases its stderr goes nowhere anyone will look. Peek follows the launcher here.
- **Resource directory name** — `resources/` vs `data/` vs `res/`.
- **xdgen adoption** — the templates generate; the shipping apps may or may not have moved (the sources disagree; see Generated XDG files).
- **`rustfmt.toml`** exists in about half the repositories.
- **Feature-list style for the libcosmic dependency** — inline array or one-comment-per-feature section.
- **Icon packaging** — one SVG per size or one scalable.
- **Metainfo install path** — `share/appdata` in the template, `share/metainfo` everywhere that is right.
- **Popup mechanism in applets** — `get_popup` in the template, `app_popup` surface actions in the shipped applets.
- **Executor type name** — `executor::Default` in the template, `executor::single::Executor` in the launcher, `SingleThreadExecutor` in applets.

---

## Adoption across the three projects

Where each project stands, and — where it diverges — why. A rejection with a reason is more useful than a follow without one.

| Convention | grabit | Locket | Peek |
|---|---|---|---|
| libcosmic unpinned, Cargo.lock committed | follows | follows | follows |
| rust-version 1.93 / rust-toolchain.toml / rustfmt.toml | follows | follows | partial — version only; rustfmt would reformat the tree |
| GPL-3.0-only | — | — | partial — `-or-later`, forced by poppler |
| justfile with rootdir / prefix / cargo-target-dir | follows; `check` is `-D warnings` | follows; install wires nothing up | follows |
| Nested `data/justfile`, vendoring recipes | skipped — nothing packages it yet | follows | follows |
| Reverse-DNS APPID across binary and metadata | follows — `io.github.idominikos.Grabit` | follows | follows |
| xdgen-generated desktop entry and metainfo | adopted, with a `CARGO_TARGET_DIR` fix | deferred until a second language exists | rejected — dependency re-resolution broke single-instance |
| Full metainfo, validated in CI | — | follows | follows — found the wrapped-MimeType bug |
| i18n-embed + Fluent + `fl!` | scoped — only what grabit itself says | follows | follows |
| `cosmic::Application` with `no_main_window` | follows | n/a — windowed | follows; own `transparent` / `is_daemon` settings |
| Single instance over D-Bus | — | follows | follows |
| KeyBind table, trait hooks instead of listeners | — | follows | — |
| cosmic-config + `watch_config` | rejected for actions — drop-in TOML is the design | follows | follows |
| Semantic theme classes and spacing tokens | partial — padding still literal | follows | follows |
| Frosted keys gate blur | — | — | follows — `frosted_system_interface` |
| `AppType::System`, keyboard nav off | — | — | follows |
| Dummy surface, create/destroy per appearance | follows | — | follows |
| Newer `surface_task` / `LiveSettings` API | — | — | stays on raw call — needs a shaped blur region |
| Activation token + scope when launching | — | follows (applet → app) | follows |
| Icon names checked along Cosmic → Pop → hicolor | — | follows — after `credit-card-symbolic` | — |
| Journal logging with stderr fallback | — | — | follows |
| debian/, flake.nix, hooks/, CI | not yet | not yet | not yet |

---

## Checklist for a new COSMIC application

For a new application, or an existing one being brought into line.

1. Generate from `cosmic-app-template` or `cosmic-applet-template`; keep the skeleton.
2. libcosmic unpinned, `Cargo.lock` committed, no separate `cosmic-config` dependency, one comment per feature.
3. `rust-toolchain.toml` and `rust-version` agree; CI takes the toolchain from the file.
4. Pick the RDNN id once. Use it for the config store, desktop entry, metainfo, icon, `StartupWMClass` and D-Bus name.
5. justfile with `rootdir` / `prefix` / `cargo-target-dir` install that wires nothing up; keep `just vendor` working; install metainfo to `share/metainfo`.
6. `i18n/`, `i18n.toml`, `src/i18n.rs`, and no user-visible string literal left in the code — including the ones you are sure will not change. Plurals through Fluent.
7. Version the config struct from the start; assume it will need a v2. Wire `watch_config` into `subscription` before you need it.
8. Shortcuts in a `KeyBind` table, surfaced through a menu bar. Implement `on_search` / `on_escape` / `on_app_exit` rather than re-listening for them.
9. Theme spacing and radii. No raw pixels, no literal colours.
10. Every icon name resolves through COSMIC → Pop → hicolor; explicit fallback chain on lookups; own icon embedded for About.
11. Gate any compositor blur on the matching `frosted_*` key.
12. Layer surfaces: `Layer::Overlay` explicit, `exclusive_zone: -1`, create/destroy per appearance, a dummy surface if all surfaces come and go, `AppType::System` + keyboard nav off.
13. If you launch applications: activation token under both env names, switcheroo GPU environment, `spawn_desktop_exec` with `desktop-systemd-scope`. All three.
14. Metainfo has `com.system76.CosmicApplication`, `requires`, `supports`, `branding`, a release matching Cargo.toml, and reachable URLs. `MimeType=` on one line.
15. `desktop-file-validate` and `appstreamcli validate` run in CI (`--no-net` on pull requests).

---

*Merged from four sets of field notes — the grabit conventions, the COSMIC 1.5.0 survey, the Locket engineering reference and the Peek conventions — all read off local checkouts on 24 August 2026: libcosmic `ef490df5`, cosmic-launcher `a9ad093`, cosmic-app-library `385db1e`, cosmic-edit `0e9c927`, cosmic-panel `3c08c30`, cosmic-ext-camera `7520175`, cosmic-app-template `97ff759`, cosmic-applet-template `58f506f`.*

*Where the four sources contradicted each other, the contradiction is kept and marked (⚡) rather than resolved by fiat. Claims marked measured were reproduced directly on cosmic-comp or against pop-launcher 1.2.7; everything else is a pattern observed in two or more independent codebases. None of this is published guidance. It is what eight codebases happen to agree on, which is a weaker claim and a more useful one.*
