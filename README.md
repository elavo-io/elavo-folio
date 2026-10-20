# Elavo Folio

Folio is the Elavo desktop. The top panel, the dock, Lens, and the app switcher are parts of Folio, and the control center will be too. Each part is its own program (`elavo-panel`, `elavo-dock`, `elavo-lens`, `elavo-switcher`), and they share code, a look, and one release. They run as GTK4 clients of stock labwc, so another system can install Folio without the Elavo image.

## What you see

**Panel.** A bar across the top of each display. The sun opens About Elavo and Software Update, both in System Settings, then System Settings itself, Sleep, Restart, Shut Down, Lock Screen, and Log Out. About and Software Update are disabled until Settings is installed. Restart, Shut Down, and Log Out ask first. The clock is centered. Network, volume, and battery sit on the right. A control center opened from those status icons comes later.

**Global menu.** To the right of the sun. The desktop, a Files window, and Folio's own dialogs are one application, Folio. Its menus are Folio, File, Edit, View, Go, Window, and Help. Files is Nautilus underneath. Items that act on the focused Files window, such as New Folder, Copy, as List, and Back, go through `extensions/nautilus-folio`. Nautilus loads that plugin. It owns `io.elavo.FolioFiles` on the session bus and calls Nautilus's own actions. It has to be installed into `nautilus/extensions-4`, and Files restarted, before those items do anything. Go's folders, Empty Trash, Undo, and Redo use D-Bus (`org.freedesktop.FileManager1` and `org.gnome.Nautilus.FileOperations2`).

Another application gets a bold menu with its name: New Window when it has a desktop file, Hide, Hide Others, Show All, and Quit. Quit asks each of its windows to close, because a window does not carry a process ID. Its own menus sit in the middle when it exports them. A Window menu follows: Minimize, Zoom, Close Window, Bring All to Front, and the window list. The panel sends those window commands with `zwlr_foreign_toplevel_handle_v1`.

labwc does not say where an application's menus are. The panel looks up the focused window's `app_id` on the session bus, reads that application's GTK menubar (`org.gtk.Menus`) and its `app.*` actions (`org.gtk.Actions`). `win.*` actions are used when the application has one window and exports one window object. A GTK application may still draw a menu bar inside its window as well.

The panel also owns `com.canonical.AppMenu.Registrar`. Qt applications that find it when they start hand their menu bar to the panel (`com.canonical.dbusmenu`) and stop drawing it in their window. The panel ties a registration to the focused window by the sender's process or Flatpak id, and finds menus already exported when it starts. A Flatpak needs `--talk-name=com.canonical.AppMenu.Registrar` to reach it. Firefox and Chromium-based browsers on Wayland, and Electron applications, get the panel's menus only.

**Dock.** A shelf along the bottom. Kept applications, then running ones, then a divider, then folders and the Trash. It magnifies, shows labels, opens stacks, and reorders by drag. Open windows come from `wlr-foreign-toplevel-management`. The panel and the dock each keep their own copy of that list, in `elavo-common`, so the list does not wait on GTK.

**Lens.** Super+Space opens a search field. Typing finds applications and, for arithmetic and unit conversion, a calculator. Return on a calculator result copies the answer. A grid lists every application.

**App switcher.** Alt+Tab lists running applications, one icon each, and releasing Alt switches to the highlighted one. Alt+` cycles the windows of the focused application and draws nothing.

**Login and lock screen.** One program, `elavo-greeter`, draws both. The selected person's wallpaper fills the screen, the time sits near the top in frosted white numerals over a blurred copy of it, and a thin bar shows the Elavo sun, network, volume, and battery. At login the sun opens Sleep, Restart, and Shut Down. Each person is a round picture and a name, with the password field under the selected one. `/etc/elavo/greeter.conf` can switch the login window to Name and Password fields instead. At login greetd runs it inside cage and it signs in through greetd's IPC. `elavo-greeter --lock` takes the screen with `ext-session-lock-v1` and checks the password with PAM service `elavo-lock`.

## Projects Folio uses

The code in this repository is Folio's. These programs and libraries stay their own projects, with their own licenses.

| Project | How Folio uses it | License |
| --- | --- | --- |
| labwc | Compositor. A distribution package. Folio ships configuration for it. | GPL-2.0-only |
| greetd | Login. A distribution package. `elavo-greeter` speaks its IPC. | GPL-3.0-only |
| cage | Hosts `elavo-greeter` at login. A distribution package. | MIT |
| Linux-PAM | The lock screen's password check. Opened at run time. | BSD-3-Clause OR GPL-2.0-or-later |
| GTK 4 | Toolkit, linked. | LGPL-2.1-or-later |
| gtk4-layer-shell | Surfaces for the panel, the dock, Lens, and the app switcher, and the lock screen's session lock (`gtk4-session-lock`). Linked. | MIT |
| serde_json | greetd's IPC messages. Linked. | MIT OR Apache-2.0 |
| gtk-rs (`gtk4`, `gio`) | Rust bindings. Linked. | MIT |
| wayland-client, wayland-protocols-wlr | The window list shared by the panel and the dock. Linked. | MIT |
| async-channel | A queue for that window list. Linked. | MIT OR Apache-2.0 |
| Nautilus | The Files window. Folio's file commands call its actions through `extensions/nautilus-folio`. | GPL-3.0-or-later |

The Trash is GVfs `trash:///`. Show in Files calls `org.freedesktop.FileManager1.ShowItems`. Sleep, Restart, and Shut Down call `org.freedesktop.login1`. Badges, progress, and attention come from the `com.canonical.Unity.LauncherEntry` signal. Those are desktop interfaces. The panel's sun, its search glass, and the dock's About tile are icon names (`elavo-sun-symbolic`, `elavo-edit-find-symbolic`, `io.elavo.Panel`) from the Elavo icon theme in `elavo-visuals`. Only the larger sun in the About window is compiled in.

## What is in this repository

| Path | What it is |
| --- | --- |
| `crates/elavo-panel` | Top panel: the sun menu, the global menu, the clock, and the status icons |
| `crates/elavo-dock` | Dock: sections, menus, stacks, drag and drop, magnification, and the window list |
| `crates/elavo-dock-settings` | The dock's settings file and its settings page, a window for now and a page Settings will embed later |
| `crates/elavo-lens` | Lens: application search, the calculator, and the Applications grid |
| `crates/elavo-switcher` | App switcher: Alt+Tab between applications, Alt+` between one application's windows |
| `crates/elavo-switcher-settings` | The switcher's settings file and the App Switcher page Settings embeds |
| `crates/elavo-common` | Desktop-file index, `app_id` matching, search ranking, launching, and the window list |
| `crates/elavo-greeter` | Login and lock screen, and `elavo-greeter-publish`, which copies a session's wallpaper for the login screen |
| `extensions/nautilus-folio` | Nautilus plugin that runs Folio's File, Edit, View, and Go actions inside Files |
| `data/systemd/user` | User units, started by `graphical-session.target` |
| `data/pam`, `data/polkit`, `data/tmpfiles` | The lock screen's PAM service, the publish helper's polkit action, and the greeter's state directory |
| `scripts/dev-run` | Build in the toolbox and run on the host session |
| `scripts/install` | Install a release build (`DESTDIR`, `PREFIX`), including the Nautilus plugin |

Each program is one Rust binary and a single-instance `GtkApplication` (`io.elavo.Panel`, `io.elavo.Dock`, `io.elavo.Lens`, `io.elavo.Switcher`). labwc owns the keys and runs the binary with a flag. The flag reaches the running instance over D-Bus, and the new process exits.

- Super+Space runs `elavo-lens --toggle`.
- Alt+Tab runs `elavo-switcher --next`, and Alt+Shift+Tab runs `--previous`. Alt+` runs `--window-next`, and Alt+Shift+` runs `--window-previous`.
- Super+1 through Super+9 run `elavo-dock --activate N`, kept applications first, then recent ones.
- Super+Ctrl+Q locks the screen, and Super+Shift+Q asks whether to log out. The image's keybinds run the panel's `lock` and `log-out` actions, the same ones as the sun menu. The power button runs `shut-down`.
- `elavo-dock --settings` opens Dock Settings, as the divider's menu does.

The session starts `graphical-session.target` from labwc's autostart. The four programs are user services wanted by that target, so a crash restarts the panel, the dock, Lens, or the app switcher. `KillMode=process` keeps applications they launched alive across a restart. Lens and the app switcher stay resident so their keys do not pay for process start.

`$XDG_CONFIG_HOME/elavo/switcher.conf` is a key file, group `[Switcher]`. The App Switcher page writes it and the switcher watches it. Extra shortcuts are also keybinds in the user's labwc `rc.xml`.

## Dock files

- `$XDG_CONFIG_HOME/elavo/dock-pins` has one desktop-file ID per line, in dock order. A missing file is seeded with Files, Firefox, and foot. Pins for applications that are not installed are kept in the file and not shown.
- `$XDG_CONFIG_HOME/elavo/dock-others` lists the files and folders after the divider: a URI per line, then tab-separated `sort=`, `display=`, and `view=` options. A missing file is seeded with Downloads.
- `$XDG_STATE_HOME/elavo/dock-recents` holds the last three applications that quit without being kept.
- `$XDG_CONFIG_HOME/elavo/dock.conf` is a key file, group `[Dock]`. The settings page writes it and the dock watches it.

## Not yet

- Lens sources beyond applications and the calculator: files, settings, web suggestions.
- Dropping an Elavo shortcut onto the dock. An `.eapp` dropped there is kept through its `launch.desktop`.
- The calendar icon's `org.elavo.CalendarIcon.Changed` reload.
- The control center, opened from the status icons.
- Menus for Firefox and Chromium-based browsers on Wayland, and for Electron applications. They publish none the panel can read, and get the panel's menus only.
- Minimize effects and window thumbnails labwc does not provide. A minimized window shows as its application's icon on a card.
- An RPM. The image copies the release binaries for now.

## License

Code in this repository is under the MIT license. See [LICENSE.md](LICENSE.md). The projects in the table above keep their own licenses.

## Develop

Build and run instructions are in [docs/develop.md](docs/develop.md).
