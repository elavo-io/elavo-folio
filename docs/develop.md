# Develop the panel and the dock

Work happens on the test PC inside a running Elavo session. Nothing here needs an image rebuild.

## One-time setup

```bash
toolbox create -r 44 elavo-dev
toolbox run -c elavo-dev sudo dnf5 install -y \
    gtk4-devel gtk4-layer-shell-devel wayland-devel \
    rust cargo clippy rustfmt gcc pkgconf-pkg-config
```

## Run a build

```bash
scripts/dev-run elavo-panel
scripts/dev-run elavo-dock
scripts/dev-run elavo-lens
```

`dev-run` builds in the toolbox, stops the running copy, and starts the new build on the host session. The host image does not carry `libgtk4-layer-shell` until the image ships the shell, so the script puts the toolbox copy on `LD_LIBRARY_PATH`.

On an image that already runs the shell as user services, stop the service first so it does not hold the D-Bus name:

```bash
systemctl --user stop elavo-dock
scripts/dev-run elavo-dock
# when done
systemctl --user start elavo-dock
```

On the phase 1 image, stop waybar first: `pkill waybar`. It returns at the next login.

## Talk to the running instance

The same binary with a flag forwards it to the running instance:

```bash
target/debug/elavo-lens --toggle
target/debug/elavo-lens --applications
target/debug/elavo-dock --activate 2
target/debug/elavo-dock --settings
```

The dock's settings live in `~/.config/elavo/dock.conf`, and the dock follows edits to it while it runs. Delete the file to go back to the defaults.

Lens takes the keyboard while it is open. Anything typed in another window in that moment lands in its search field.

Until the image ships Lens, point Super+Space at the dev build in `~/.config/labwc/rc.xml` and run `labwc --reconfigure`:

```xml
<keybind key="W-space">
  <action name="Execute" command="sh -c 'LD_LIBRARY_PATH=$HOME/GitHub/elavo-shell/target/devlib exec $HOME/GitHub/elavo-shell/target/debug/elavo-lens --toggle'" />
</keybind>
```

## Debug output

```bash
G_MESSAGES_DEBUG=elavo-dock scripts/dev-run elavo-dock
```

prints each window's `app_id` and the desktop ID it matched. Use it when a running application does not group with its pin. `GTK_DEBUG=interactive` opens the GTK inspector.

## Checks before a pull request

```bash
toolbox run -c elavo-dev sh -c 'cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace'
```

CI runs the same commands in a `fedora:44` container.

## Screenshots

`grim -g "X,Y WxH" out.png` with logical coordinates. Do not pass `-s`; scaled captures drop the panel's translucency and look black.
