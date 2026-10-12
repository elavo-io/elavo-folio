# elavo-shell

The Elavo desktop session: the top panel, the dock, and Lens, as GTK4 layer-shell clients of stock labwc. Later this repository also takes the `greetd` and labwc session files from the image repository, and the control center.

The behavior is specified in the private planning repository:

- `planning/desktop/top-panel.md`
- `planning/desktop/dock.md`
- `planning/desktop/lens.md`
- `planning/desktop/session-and-labwc.md`
- `planning/decisions/002-labwc-and-shell-split.md`

## Layout

| Path | What it is |
| --- | --- |
| `crates/elavo-panel` | Top panel: the Elavo menu (the sun), clock, network/volume/battery icons |
| `crates/elavo-dock` | Dock: the Mac dock's sections, menus, stacks, drag and drop, and magnification; windows over `wlr-foreign-toplevel-management` |
| `crates/elavo-dock-settings` | The dock's settings file and its settings page, shown in a window now and embedded by Settings later |
| `crates/elavo-lens` | Lens: search for applications and a calculator, and the Applications grid |
| `crates/elavo-common` | Desktop-file index across the XDG and Flatpak export dirs, `app_id` matching, search ranking, launching |
| `data/systemd/user` | User units, started by `graphical-session.target` |
| `scripts/dev-run` | Build in the toolbox, run on the host session |
| `scripts/install` | Install a release build (`DESTDIR`, `PREFIX`) |

## Decisions

- **Language.** Rust, with `gtk4`, `gtk4-layer-shell`, and `wayland-client`. One binary per program.
- **Keybinds.** labwc owns the keys and runs the program with a flag. Each program is a single-instance `GtkApplication` (`io.elavo.Panel`, `io.elavo.Dock`, `io.elavo.Lens`), so the flag reaches the running instance over D-Bus and the new process exits.
  - Super+Space: `elavo-lens --toggle`
  - Super+1 through Super+9: `elavo-dock --activate N` (kept applications first, then recent ones)
  - `elavo-dock --settings` opens Dock Settings, as the divider's menu does.
- **Startup.** The session starts `graphical-session.target` from labwc's `autostart`. All three programs are user services wanted by that target, so a crash restarts them instead of leaving the session without a panel. `KillMode=process` keeps launched applications alive across a restart. Lens stays resident so Super+Space does not pay for process start and the desktop-file scan.
- **Window list.** The dock opens its own Wayland connection on a worker thread for `zwlr_foreign_toplevel_manager_v1`, so that queue never waits on GDK's.
- **The dock's surface.** One layer surface per output spans the dock's edge and is as deep as the largest dock with its labels. The dock draws icons itself (`DockView`), sets the input region to the shelf, and sets the exclusive zone by hand, so magnification and labels cover windows without moving them. The depth never changes, because compositors report pointer positions against the old size until the pointer moves.
- **Dock settings.** `$XDG_CONFIG_HOME/elavo/dock.conf` (key file, group `[Dock]`) is the only interface between the settings page and the dock: the page writes it and the dock watches it. Settings will embed `elavo_dock_settings::page()` instead of the window.
- **Linux stand-ins for Mac dock features.** Badges, progress, and attention bounce come from the `com.canonical.Unity.LauncherEntry` signal. The Trash is GVfs `trash:///`. Show in Files calls `org.freedesktop.FileManager1.ShowItems`. Open at Login writes an XDG autostart entry, which needs the session to start `xdg-desktop-autostart.target`.
- **The Elavo menu.** The sun opens About Elavo, System Settings (disabled until Settings exists), Sleep, Restart, Shut Down, Lock Screen, and Log Out. Power actions call `org.freedesktop.login1` on the system bus. Restart, Shut Down, and Log Out ask first. Lock Screen runs `loginctl lock-session`, which the session's `swayidle` answers with `swaylock`.
- **Lens.** A full-screen overlay layer surface with exclusive keyboard focus and a centered column. A click outside the column closes it; layer shell gives no other signal for a click on another client. Typing shows a Calculator section (arithmetic and unit conversion, Return copies the answer), a Top Hit, and Applications. The grid button, Ctrl+1, or `--applications` shows every application. Super+1 is not used because labwc gives it to the dock.
- **Styling.** Each program carries a small stand-in stylesheet at application priority until the Elavo GTK theme ships. The CSS class names (`elavo-panel-bar`, `elavo-dock-bar`, `elavo-dock-dot`, `elavo-dock-separator`, `elavo-dock-label`, `elavo-dock-badge`, `elavo-dock-menu`, `elavo-dock-stack`, `elavo-lens-bar`, and the rest) are the theme's contract.
- **The sun.** The panel looks up `elavo-sun-symbolic` in the icon theme. A copy of `elavo-visuals` `icons/unsorted/symbol/elavo-sun.svg` (commit `95273b0`) is compiled in as a fallback.

## Dock files

- `$XDG_CONFIG_HOME/elavo/dock-pins` has one desktop-file ID per line, in dock order. A missing file is seeded with Files, Firefox, and foot. Pins for applications that are not installed are kept in the file and not shown.
- `$XDG_CONFIG_HOME/elavo/dock-others` lists the files and folders after the divider: a URI per line, then tab-separated `sort=`, `display=`, and `view=` options. A missing file is seeded with Downloads.
- `$XDG_STATE_HOME/elavo/dock-recents` holds the last three applications that quit without being kept.
- `$XDG_CONFIG_HOME/elavo/dock.conf` holds the settings; see above.

## Not yet

- Lens sources beyond applications and the calculator: files, settings, web suggestions.
- Dropping an Elavo shortcut onto the dock (needs the Applications folder, phase 3). An `.eapp` dropped there is kept through its `launch.desktop`.
- The calendar icon's `org.elavo.CalendarIcon.Changed` reload.
- Control center from the status icons (phase 3), the global menu (phase 5).
- Dock features labwc and its protocols cannot carry: the Genie and Scale minimize effects and window thumbnails (no screencopy of a single window; minimized windows show as their app's icon on a card), Option and Command clicks (the dock takes no keyboard), Force Quit (no process ID per window), Show All Windows, and the poof when an icon is dragged off.
- An RPM. The image copies the release binaries for now.

See [docs/develop.md](docs/develop.md) to build and run.
