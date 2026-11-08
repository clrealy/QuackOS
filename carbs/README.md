# Carbs 🍞

The Wayland compositor for QuackOS. Written in Rust on [Smithay](https://github.com/Smithay/smithay).
Bread (the desktop: dock, top bar, launcher) runs on top of it.

## Status: step 1-4 of the roadmap
- [x] nested window (runs inside your current desktop - safe to develop)
- [x] xdg-shell app windows, cascade placement, popups tracked
- [x] pointer + keyboard + scroll input, click-to-focus + raise
- [x] move / resize via pointer grabs, close
- [ ] minimize (needs somewhere to minimize *to* - waiting on Bread's dock)
- [ ] real hardware backend (DRM/KMS + libinput + libseat)
- [ ] layer-shell for Bread's dock/top bar
- [ ] XWayland

## Window handling
| | |
|---|---|
| click | focus + raise |
| Super + left drag | move the window under the pointer |
| Super + right drag | resize it, from whichever corner you started nearest |
| Super + Q | ask the focused window to close |

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

## Run
    cargo run --release                       # empty nested Carbs window
    cargo run --release -- -c weston-terminal # ...and launch an app inside it
    RUST_LOG=debug cargo run                  # chatty logs
