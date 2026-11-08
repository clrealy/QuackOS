# Roadmap

## Done
- [x] Base decision: Ubuntu 26.04 LTS, built with Cubic
- [x] Recovery partition protection (hidden, unmounted, kernel read-only via udev)
- [x] No Snap (purge + apt pin), Firefox from Mozilla's apt repo, Flatpak + Flathub
- [x] `peck` front-end, wired to GURT's real verbs
- [x] GURT preinstalled by `main.sh`
- [x] DuckFS core + FUSE mount + 12 unit tests
- [x] Carbs: nested compositor, xdg-shell windows, pointer/keyboard/scroll, click-to-focus
- [x] Carbs: interactive move/resize (pointer grabs) + close, client- and Super-driven
- [x] Carbs: a cursor you can see (client surfaces + Carbs' own arrow)
- [x] Carbs: maximize, fullscreen, alt-tab, off-screen clamping, config file
- [x] Carbs: xdg-decoration + primary selection

## Next
- [ ] Carbs: layer-shell (so Bread's dock/top bar can exist)
- [ ] Carbs: XWayland, so X11-only apps run
- [ ] Carbs: DRM/KMS + libinput + libseat, so it runs on real hardware at all
- [ ] Carbs: minimize (needs Bread's dock first, otherwise there's nowhere to go)
- [ ] Beak: the QuackOS installer
- [ ] fastfetch logo + os-release branding
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
