# Carbs 🍞

The Wayland compositor for QuackOS. Written in Rust on [Smithay](https://github.com/Smithay/smithay).
Bread (the desktop: dock, top bar, launcher) runs on top of it.

## Status
- [x] nested window (runs inside your current desktop - safe to develop)
- [x] xdg-shell app windows, cascade placement, popups tracked
- [x] pointer + keyboard + scroll input, click-to-focus + raise
- [x] a visible cursor: client cursor surfaces, and Carbs' own arrow elsewhere
- [x] move / resize via pointer grabs, close
- [x] maximize, fullscreen, alt-tab, and windows that can't be lost off-screen
- [x] a config file (keyboard layout, shortcuts' commands, colours, autostart)
- [x] xdg-decoration and primary selection (middle-click paste)
- [ ] minimize (needs somewhere to minimize *to* - waiting on Bread's dock)
- [ ] layer-shell for Bread's dock/top bar
- [ ] XWayland
- [ ] real hardware backend (DRM/KMS + libinput + libseat)

## Window handling
| | |
|---|---|
| click | focus + raise |
| Super + left drag | move the window under the pointer |
| Super + right drag | resize it, from whichever corner you started nearest |
| Super + Up | maximize / restore |
| Super + F | fullscreen / restore |
| Alt + Tab | next window (repeat to rotate through all of them) |
| Alt + Shift + Tab | previous window - exactly undoes Alt+Tab |
| Super + Return | terminal |
| Super + D | launcher |
| Super + Q | ask the focused window to close |
| Super + Shift + E | end the session |

Dragging a maximized window restores it under the pointer. A window can never
be dragged fully off-screen: at least 72px stays reachable, and the top edge
never goes above the work area.

Apps that draw their own frame (GTK, weston-terminal, ...) can also drive moves and
resizes themselves - dragging their titlebar or frame edge goes through
`xdg_toplevel.move` / `.resize`, and Carbs honours those too. The Super bindings are
there for apps with no decorations, since Carbs doesn't draw any yet.

Carbs never kills a client: Super+Q sends `xdg_toplevel.close`, which the app may
answer with a "save your work?" dialog or ignore entirely.

## Build deps (Ubuntu)
    sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev \
        libegl-dev libgles-dev libgl-dev libx11-dev
    # later, for the real-hardware backend:
    # sudo apt install libudev-dev libgbm-dev libinput-dev libseat-dev libdisplay-info-dev

## Config
Copy `carbs.conf.example` to `~/.config/carbs/carbs.conf`. It covers the
keyboard layout, what the shortcuts run, the background colour, cursor size and
autostart. A broken line is logged and skipped - it never stops the desktop.

## Run
    cargo run --release                       # empty nested Carbs window
    cargo run --release -- -c weston-terminal # ...and launch an app inside it
    RUST_LOG=debug cargo run                  # chatty logs
