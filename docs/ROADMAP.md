# Roadmap

## Done
- [x] Base decision: Ubuntu 26.04 LTS, built with Cubic
- [x] Recovery partition protection (hidden, unmounted, kernel read-only via udev)
- [x] No Snap (purge + apt pin), Firefox from Mozilla's apt repo, Flatpak + Flathub
- [x] `peck` front-end, wired to GURT's real verbs
- [x] GURT preinstalled by `main.sh`
- [x] DuckFS core + FUSE mount + 12 unit tests
- [x] Carbs: nested compositor, xdg-shell windows, pointer/keyboard/scroll, click-to-focus

## Next
- [ ] Carbs: move/resize (pointer grabs), close/minimize, macOS-style floating feel
- [ ] Carbs: layer-shell (so Bread's dock/top bar can exist)
- [ ] Bread: dock, top bar, launcher, settings
- [ ] Recovery environment itself ("The Pond"): small image on the RECOVERY partition that can reinstall from a manifest
- [ ] First real Cubic build and VM boot test
- [ ] Branding: own logo (a duck, drawn fresh, no Apple lookalikes), wallpapers, boot splash, boot sound

## Later
- [ ] Carbs: real hardware backend (DRM/KMS + libinput + libseat), XWayland
- [ ] DuckFS: symlinks, hard links, xattrs, journaling (crash safety)
- [ ] Pin GURT to a release tag in `main.sh` for reproducible ISOs
- [ ] Check the name "QuackOS" for existing projects/trademarks before going public

## Ideas (not decided)
- Recovery mode called "The Pond"
- Boot sound is a quack
