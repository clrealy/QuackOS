#!/usr/bin/env bash
# Safely dry-run build/main.sh against a throwaway fake root (no apt, no downloads, no real changes).
# main.sh only writes config files when ROOT is set; the apt/Flatpak/GURT steps are skipped.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
FAKE="$(mktemp -d)"
trap 'rm -rf "$FAKE"' EXIT

mkdir -p "$FAKE/etc"
printf 'LABEL=RECOVERY /rec ext4 defaults 0 0\nLABEL=root / ext4 defaults 0 1\n' > "$FAKE/etc/fstab"

ROOT="$FAKE" bash "$HERE/build/main.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
grep -q RECOVERY "$FAKE/etc/fstab"                              && fail "RECOVERY still in fstab"
[[ -f "$FAKE/etc/udev/rules.d/99-recovery-protect.rules" ]]     || fail "udev rule missing"
grep -q 'blockdev --setro' "$FAKE/etc/udev/rules.d/99-recovery-protect.rules" || fail "read-only rule missing"
[[ -f "$FAKE/etc/apt/preferences.d/nosnap.pref" ]]              || fail "nosnap pin missing"
[[ -f "$FAKE/etc/apt/preferences.d/mozilla" ]]                  || fail "mozilla pin missing"
[[ -f "$FAKE/etc/apt/sources.list.d/mozilla.sources" ]]         || fail "mozilla source missing"
[[ -x "$FAKE/usr/local/bin/peck" ]]                             || fail "peck not installed"
diff -q "$FAKE/usr/local/bin/peck" "$HERE/peck/peck" >/dev/null || fail "peck embedded in main.sh differs from peck/peck (re-sync them)"
echo "OK: main.sh fake-root checks passed"
