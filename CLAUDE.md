# CLAUDE.md - QuackOS

Instructions for Claude Code working in this repo. Read this first.

## What this project is
QuackOS is a macOS-inspired Linux distro (its own identity, no Apple branding or lookalike assets) on **Ubuntu 26.04 LTS**, built with **Cubic**.
Owner: clrealy. They like a casual tone; keep messages short and friendly.

## Hard rules (the owner decided these)
- **No Snap**, ever. `snapd` is purged and pinned out. Firefox = Mozilla apt repo; apps = Flatpak or GURT.
- **The recovery partition must not be deletable** from the running OS. That is the only restriction on the OS.
  (Earlier ideas about a "9 prompts to delete recovery" boss fight were dropped. Do not bring them back.)
- Package manager command is `peck`; backend is **GURT** (github.com/clrealy/GURT). Don't reimplement GURT.
- Names: distro **QuackOS**, desktop **Bread**, compositor/WM **Carbs**, filesystem **DuckFS**.
- Original art only: never copy Apple/macOS icons, wallpapers, fonts or logos. A duck theme is welcome.

## Layout
- `build/main.sh` - the ONE script run inside the Cubic chroot as root. Idempotent-ish, installs everything below.
- `peck/peck` - bash front-end for GURT. **`build/main.sh` embeds a copy of it** (a `PECKEOF` heredoc). After editing `peck/peck`, re-embed it; `tools/test-main-fakeroot.sh` fails if they differ.
- `duckfs/` - Rust crate. `lib.rs` = the filesystem (4 KiB blocks, inodes, bitmaps), `mount.rs` = FUSE glue (`fuser`), `main.rs` = CLI (`mkfs`, `mount`, `info`).
- `carbs/` - Rust crate on Smithay 0.7. `state.rs` (compositor state, wayland socket), `handlers.rs` (protocol delegates), `input.rs` (pointer/keyboard, focus), `winit.rs` (nested backend + render loop).
- `tools/test-main-fakeroot.sh` - dry-runs `main.sh` against a fake root.
- `docs/` - ROADMAP.md, BUILDING.md.

## How to test (do this before saying anything works)
| Thing | Command |
|---|---|
| main.sh config logic | `./tools/test-main-fakeroot.sh` (sets `ROOT=` so apt/network steps are skipped) |
| peck | `bash -n peck/peck`, and run with a fake `gurt` on PATH (see below) |
| DuckFS | `cd duckfs && cargo test` ; FUSE smoke test: `duckfs mkfs a.img 32 && duckfs mount a.img /tmp/m &` then cp/mkdir/rm/mv, then `fusermount -u /tmp/m` |
| Carbs | headless: `Xvfb :77 -screen 0 1024x640x24 +extension GLX &` then `DISPLAY=:77 LIBGL_ALWAYS_SOFTWARE=1 XDG_RUNTIME_DIR=/tmp/xdg cargo run -- -c weston-simple-shm`, screenshot with `import -window root shot.png` |

Fake gurt for peck tests: a script on PATH named `gurt` that does `echo "GURT GOT: $*"`.

Carbs build deps (Ubuntu): `build-essential pkg-config libwayland-dev libxkbcommon-dev libegl-dev libgles-dev libgl-dev libx11-dev`. Test client: `weston` package.
Real-hardware backend will later need: `libudev-dev libgbm-dev libinput-dev libseat-dev libdisplay-info-dev`.

## Known gaps / honest status
- `build/main.sh` has **never been run inside a real Cubic chroot**. Only its config-writing half is tested (fake root). The apt, Flatpak and GURT steps are untested here.
- The GURT step clones GURT's `main` at build time (not pinned).
- No recovery *environment* exists yet; only the protection (udev read-only + hidden + not in fstab). It needs a partition labeled `RECOVERY` (fs label or GPT partition name). True hardware-level protection would need firmware support; root can `blockdev --setrw`.
- DuckFS has no journal: a crash mid-write can corrupt an image. No symlinks/hard links/xattrs yet.
- Carbs only runs nested (winit). Keyboard input reaching clients was never verified, only that input doesn't crash it. No move/resize/close yet, no layer-shell, no XWayland.
- Bread does not exist yet.

## Conventions
- Rust: edition 2021, `cargo fmt`, keep `cargo test` green. Prefer small modules.
- Shell: `set -euo pipefail`, quote everything, `bash -n` before committing.
- Never run destructive commands (`rm -rf /`, `dd`, `mkfs`, `wipefs`) on the host. Use images and fake roots.
- When something is unverified, say so in the commit/PR text and in docs. Don't claim it works unless you ran it.
- Commit messages: short imperative subject. Keep `docs/ROADMAP.md` checkboxes honest.

## Good next tasks (in order)
1. Carbs: move/resize/close with pointer grabs (Smithay `PointerGrab`), floating windows, focus rules.
2. Carbs: `wlr-layer-shell` so Bread's dock/top bar can attach.
3. Bread: a first panel (top bar) as a separate Rust client using layer-shell.
4. Recovery environment ("The Pond"): minimal initramfs/squashfs on the RECOVERY partition + reinstall-from-manifest.
5. Run `build/main.sh` in a real Cubic chroot (in a VM), fix what breaks, then pin GURT to a tag.
