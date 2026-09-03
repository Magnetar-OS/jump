# Name of the application's binary.
name := 'jump'
# The unique ID of the application, matching `App::APP_ID`.
appid := 'dev.entro314labs.Jump'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
bin-src := cargo-target-dir / 'release' / name
bin-dst := base-dir / 'bin' / name
applet-src := cargo-target-dir / 'release' / (name + '-applet')
applet-dst := base-dir / 'bin' / (name + '-applet')
settings-src := cargo-target-dir / 'release' / (name + '-settings')
settings-dst := base-dir / 'bin' / (name + '-settings')
applet-desktop-dst := base-dir / 'share' / 'applications' / (appid + 'Applet.desktop')
settings-desktop-dst := base-dir / 'share' / 'applications' / (appid + 'Settings.desktop')
desktop-dst := base-dir / 'share' / 'applications' / (appid + '.desktop')
autostart-dst := base-dir / 'share' / 'applications' / (appid + '.Autostart.desktop')
systemd-dst := base-dir / 'lib' / 'systemd' / 'user' / (name + '.service')
metainfo-dst := base-dir / 'share' / 'metainfo' / (appid + '.metainfo.xml')
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
license-dst := base-dir / 'share' / 'licenses' / name / 'LICENSE'

# Icons ship one SVG per size rather than a single scalable file, matching what
# every COSMIC application does — it lets the small sizes be drawn on the pixel
# grid instead of being scaled down from a detailed drawing.
icon-sizes := '16x16 24x24 32x32 48x48 64x64 128x128 256x256'

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --workspace --all-targets {{args}} -- -W clippy::pedantic

# Runs the test suite
test *args:
    cargo test --workspace {{args}}

# Runs the performance budgets with their timings printed. The same test
# binary CI runs, so a budget that holds here holds there; the output is the
# table to paste into a performance discussion.
bench *args:
    cargo test --release -p jump-core --test budgets {{args}} -- --nocapture --test-threads=1

# Validates the desktop entries and the AppStream metainfo. Nothing else
# checks them: the compiler never sees these files, and the first thing that
# does is a software centre. `--no-net` so the check passes without reaching
# the URLs; run `appstreamcli validate` by hand for the networked pass.
validate:
    desktop-file-validate data/applications/*.desktop
    appstreamcli validate --no-net data/metainfo/*.metainfo.xml

# Vendors dependencies so packaged and offline builds can resolve the git deps
vendor:
    #!/usr/bin/env bash
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Installs files into the system
install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0755 {{applet-src}} {{applet-dst}}
    install -Dm0755 {{settings-src}} {{settings-dst}}
    install -Dm0644 data/applications/{{appid}}.desktop {{desktop-dst}}
    install -Dm0644 data/applications/{{appid}}Applet.desktop {{applet-desktop-dst}}
    install -Dm0644 data/applications/{{appid}}Settings.desktop {{settings-desktop-dst}}
    install -Dm0644 data/applications/{{appid}}.Autostart.desktop {{autostart-dst}}
    install -Dm0644 data/systemd/jump.service {{systemd-dst}}
    install -Dm0644 data/metainfo/{{appid}}.metainfo.xml {{metainfo-dst}}
    install -Dm0644 LICENSE {{license-dst}}
    for size in {{icon-sizes}}; do \
        install -Dm0644 "data/icons/hicolor/$size/apps/{{appid}}.svg" \
            "{{icons-dst}}/$size/apps/{{appid}}.svg"; \
    done
    # Both are caches; neither notices a new file on its own. Skipped when
    # staging into a package root, where the distro's hooks run them instead.
    if [ -z "{{rootdir}}" ]; then \
        update-desktop-database "{{base-dir}}/share/applications" || true; \
        gtk-update-icon-cache -q -t "{{icons-dst}}" || true; \
    fi

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{applet-dst}} {{settings-dst}} {{desktop-dst}} {{applet-desktop-dst}} \
          {{settings-desktop-dst}} {{autostart-dst}} {{systemd-dst}} {{metainfo-dst}} {{license-dst}}
    for size in {{icon-sizes}}; do \
        rm -f "{{icons-dst}}/$size/apps/{{appid}}.svg"; \
    done

# Installs into the current user's home rather than the system
install-user:
    install -Dm0755 {{bin-src}} ~/.local/bin/{{name}}
    install -Dm0755 {{applet-src}} ~/.local/bin/{{name}}-applet
    install -Dm0755 {{settings-src}} ~/.local/bin/{{name}}-settings
    install -Dm0644 data/applications/{{appid}}.desktop ~/.local/share/applications/{{appid}}.desktop
    install -Dm0644 data/applications/{{appid}}Applet.desktop ~/.local/share/applications/{{appid}}Applet.desktop
    install -Dm0644 data/applications/{{appid}}Settings.desktop ~/.local/share/applications/{{appid}}Settings.desktop
    install -Dm0644 data/systemd/jump.service ~/.config/systemd/user/{{name}}.service
    install -Dm0644 data/metainfo/{{appid}}.metainfo.xml ~/.local/share/metainfo/{{appid}}.metainfo.xml
    for size in {{icon-sizes}}; do \
        install -Dm0644 "data/icons/hicolor/$size/apps/{{appid}}.svg" \
            ~/.local/share/icons/hicolor/"$size"/apps/{{appid}}.svg; \
    done
    update-desktop-database ~/.local/share/applications || true
    gtk-update-icon-cache -q -t ~/.local/share/icons/hicolor || true
    @echo
    @echo 'Installed to ~/.local/bin/{{name}}.'
    @echo 'Start it now:      systemctl --user enable --now {{name}}.service'
    @echo 'Bind a shortcut to `{{name}}` in Settings > Keyboard > Shortcuts.'

# Removes a user installation
uninstall-user:
    -systemctl --user disable --now {{name}}.service
    rm -f ~/.local/bin/{{name}} \
          ~/.local/bin/{{name}}-applet \
          ~/.local/bin/{{name}}-settings \
          ~/.local/share/applications/{{appid}}.desktop \
          ~/.local/share/applications/{{appid}}Applet.desktop \
          ~/.local/share/applications/{{appid}}Settings.desktop \
          ~/.config/systemd/user/{{name}}.service \
          ~/.local/share/metainfo/{{appid}}.metainfo.xml
    for size in {{icon-sizes}}; do \
        rm -f ~/.local/share/icons/hicolor/"$size"/apps/{{appid}}.svg; \
    done
