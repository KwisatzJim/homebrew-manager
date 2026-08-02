# Homebrew Manager

A native GUI for managing [Homebrew](https://brew.sh), built in Rust with
[eframe/egui](https://github.com/emilk/egui).

<img width="1092" height="784" alt="Screenshot 2026-08-02 at 7 33 58 AM" src="https://github.com/user-attachments/assets/4afbea14-2061-4ee2-9960-33def990ffb8" />


## Features

- **Status** — detects whether `brew` is installed (checks `PATH`, then
  `/opt/homebrew`, `/usr/local`, and Linuxbrew locations), shows version and
  prefix, and installs Homebrew for you if it's missing (runs the official
  installer with `NONINTERACTIVE=1`).
- **Installed** — lists installed formulae and casks with versions, filter
  box, multi-select uninstall/upgrade, `brew info` popup per package.
- **Search / Install** — `brew search`, multi-select install (formula or
  cask), inline install/info buttons.
- **Updates** — `brew update`, `brew outdated` (shows current → latest,
  pinned status), upgrade all or selected, pin/unpin.
- **Maintenance** — `brew cleanup -s`, `brew autoremove`, `brew doctor`,
  `brew update`, one click each.
- Live streaming console at the bottom of the window shows real stdout/stderr
  from every command as it runs, not just a final result.

## Project layout

```
src/brew.rs  -- all brew-CLI / process-spawning logic (no GUI deps, unit tested)
src/app.rs   -- eframe::App implementation and UI
src/main.rs  -- entry point
```

`brew.rs` has no GUI dependencies and includes unit tests for its output
parsers:

```
cargo test --lib brew
```

## Building

Requires a reasonably current Rust toolchain (install via
[rustup](https://rustup.rs) if you don't already have one — the code targets
current stable and needs a toolchain new enough to build `winit`/`wgpu`,
which on Linux effectively means anything from the last ~2 years).

```
cargo build --release
```

On Linux you'll need the usual GUI dev headers if they aren't already
present (CachyOS should have most of these via its desktop meta-packages):

```
# Arch/CachyOS
sudo pacman -S libx11 libxkbcommon wayland libxrandr libxi libxcursor libxinerama mesa

# Debian/Ubuntu
sudo apt install libx11-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxrandr-dev libxi-dev libxcursor-dev libxinerama-dev libgl1-mesa-dev
```

On macOS, no extra system deps are needed beyond Xcode Command Line Tools.

The resulting binary is at `target/release/homebrew-manager`.

## Notes / limitations

- The "Install Homebrew" button runs the official install script
  (`curl -fsSL .../install.sh | bash`) with `NONINTERACTIVE=1`, which skips
  the "press RETURN" prompt. The installer can still invoke `sudo` for
  certain one-time setup steps (e.g. Xcode Command Line Tools on macOS) —
  if it appears to hang, it's likely waiting on a password prompt with
  nowhere to show it; in that case just run the installer from a real
  terminal once, and the app will pick up the existing installation
  afterward.
- "Search" available packages uses `brew search`, which only returns
  matching formula/cask *names* (Homebrew doesn't expose a way to browse
  its entire catalog offline) — this mirrors what `brew search <term>` gives
  you on the command line.
- All mutating operations (install/uninstall/upgrade/cleanup/etc.) stream
  live output into the console panel so you can see exactly what Homebrew
  is doing, same as running it in a terminal.
