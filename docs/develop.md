# Develop Folio

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
scripts/dev-run elavo-switcher
```

`dev-run` builds in the toolbox, stops the running copy, and starts the new build on the host session. The host image does not carry `libgtk4-layer-shell` until the image ships Folio, so the script puts the toolbox copy on `LD_LIBRARY_PATH`.

On an image that already runs Folio as user services, stop the service first so it does not hold the D-Bus name:

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
  <action name="Execute" command="sh -c 'LD_LIBRARY_PATH=$HOME/GitHub/elavo-folio/target/devlib exec $HOME/GitHub/elavo-folio/target/debug/elavo-lens --toggle'" />
</keybind>
```

## Try the greeter

```bash
scripts/dev-run elavo-greeter --demo
scripts/dev-run elavo-greeter --demo --show name
scripts/dev-run elavo-greeter --demo --lock
scripts/dev-run elavo-greeter --demo --demo-people 3
```

`--demo` draws the login or lock screen in an ordinary window and accepts the password `elavo`. Nothing signs in or unlocks. `--demo-people N` adds up to eight made-up people to the login list, to check the row with several accounts.

Test the real lock only in a nested labwc with an empty config, so its autostart does not start a second set of session services:

```bash
mkdir -p /tmp/greeter-labwc
printf '<?xml version="1.0"?>\n<labwc_config></labwc_config>\n' > /tmp/greeter-labwc/rc.xml
labwc -C /tmp/greeter-labwc -s "$PWD/target/debug/elavo-greeter --lock" &
```

A lock client that dies leaves that compositor locked, so stop the nested labwc by its own PID afterwards. The lock also sets `LockedHint` on your real session, because it finds the session through your user. Clear it when done:

```bash
s=$(busctl get-property org.freedesktop.login1 /org/freedesktop/login1/user/self org.freedesktop.login1.User Display | awk '{print $3}' | tr -d '"')
busctl call org.freedesktop.login1 "$s" org.freedesktop.login1.Session SetLockedHint b false
```

Unlocking needs `/etc/pam.d/elavo-lock`, which the image installs.

`ELAVO_GREETER_TRACE=1` prints each startup step's time on stderr, from process start to the first painted frame. A `--lock` child counts from its parent's start.

To see the login screen at 4K without changing greetd, run it in a headless cage. The greeter scales the output to 175% before its window appears:

```bash
cat > /tmp/cage-greeter.sh <<'EOF'
#!/bin/sh
wlr-randr --output HEADLESS-1 --custom-mode 3840x2160
"$HOME/GitHub/elavo-folio/target/debug/elavo-greeter" & g=$!
sleep 4; grim /tmp/login-4k.png; kill $g
EOF
chmod +x /tmp/cage-greeter.sh
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 cage -- /tmp/cage-greeter.sh
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
