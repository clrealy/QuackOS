# QuackOS 🦆

A macOS-inspired (but its own thing) Linux distro based on **Ubuntu 26.04 LTS**, with a recovery partition that survives `rm -rf /`.

> Status: early. Several pieces exist and are tested in isolation; nothing has been booted end-to-end yet. See [docs/ROADMAP.md](docs/ROADMAP.md).

## The pieces

| Piece | What it is | Where | State |
|---|---|---|---|
| **QuackOS** | The distro (Ubuntu 26.04 base, built in Cubic) | `build/` | script written, not yet run in a real Cubic chroot |
| **Recovery** | Hidden, never-mounted, kernel read-only `RECOVERY` partition (macOS Recovery style) | `build/main.sh` | protection done, recovery *environment* not built |
| **peck** | The package manager command, a front-end for GURT | `peck/` | works against real GURT |
| **GURT** | The backend package tool (separate repo: [clrealy/GURT](https://github.com/clrealy/GURT)) | installed by `main.sh` | external |
| **DuckFS** | A real block filesystem in Rust, mounts via FUSE | `duckfs/` | 12 tests pass, FUSE mount tested |
| **Carbs** | The Wayland compositor (Rust + Smithay) | `carbs/` | nested window only (runs inside another desktop) |
| **Bread** | The desktop environment on top of Carbs: dock, top bar, launcher | not started | |

No Snap. Firefox comes from Mozilla's apt repo, other apps from Flatpak or GURT.

## Quick start

Want Claude Code to build on this? Read [docs/BUILDING.md](docs/BUILDING.md).

Try the pieces:

```bash
# peck (needs GURT installed)
./peck/peck help

# DuckFS
cd duckfs && cargo test && cargo run --release -- mkfs /tmp/quack.img 64

# Carbs (nested; safe, it can't touch your real session)
sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev libegl-dev libgles-dev libgl-dev libx11-dev
cd carbs && cargo run --release -- -c weston-terminal

# main.sh config logic, against a fake root (no real changes)
./tools/test-main-fakeroot.sh
```

## Safety

`build/main.sh` is meant for a **Cubic chroot**, not your real machine. Don't run it on a system you care about.
Test anything destructive in a VM.

## License

TBD.
