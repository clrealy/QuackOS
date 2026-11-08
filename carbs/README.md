# Carbs 🍞

The Wayland compositor for QuackOS. Written in Rust on [Smithay](https://github.com/Smithay/smithay).
Bread (the desktop: dock, top bar, launcher) runs on top of it.

## Status: step 1-3 of the roadmap
- [x] nested window (runs inside your current desktop - safe to develop)
- [x] xdg-shell app windows, cascade placement, popups tracked
- [x] pointer + keyboard + scroll input, click-to-focus + raise
- [ ] move / resize (grabs), close/minimize
- [ ] real hardware backend (DRM/KMS + libinput + libseat)
- [ ] layer-shell for Bread's dock/top bar
- [ ] XWayland

## Build deps (Ubuntu)
    sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev \
        libegl-dev libgles-dev libgl-dev libx11-dev
    # later, for the real-hardware backend:
    # sudo apt install libudev-dev libgbm-dev libinput-dev libseat-dev libdisplay-info-dev

## Run
    cargo run --release                       # empty nested Carbs window
    cargo run --release -- -c weston-terminal # ...and launch an app inside it
    RUST_LOG=debug cargo run                  # chatty logs
