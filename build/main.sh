#!/usr/bin/env bash
# main.sh - run inside the Cubic chroot terminal (as root, with internet).
# 1. Makes the RECOVERY partition undeletable from the normal OS
#    (never mounted, hidden, kernel-level read-only)
# 2. Removes Snap for good
# 3. Installs Firefox from Mozilla's apt repo + Flatpak (Flathub) for apps
# 4. Installs `peck`, the QuackOS package manager (front-end for GURT)
# 5. Installs GURT itself (the backend peck talks to)
# Needs a partition labeled RECOVERY (filesystem label or GPT partition name).

set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

ROOT="${ROOT:-}"   # only set this for testing against a fake root
[[ -n "$ROOT" || $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }
LIVE=0; [[ -z "$ROOT" ]] && LIVE=1   # 1 = really in the chroot, run apt etc.

# ---------- 0. clean up the old rm wrapper if it was ever installed ----------
if (( LIVE )) && dpkg-divert --list /usr/bin/rm | grep -q rm.real; then
  rm -f /usr/bin/rm
  dpkg-divert --remove --rename /usr/bin/rm
fi

# ---------- 1. protect RECOVERY ----------
if [[ -f "$ROOT/etc/fstab" ]]; then
  sed -i '/LABEL=RECOVERY/d' "$ROOT/etc/fstab"
fi

install -d "$ROOT/etc/udev/rules.d"
cat > "$ROOT/etc/udev/rules.d/99-recovery-protect.rules" <<'RULES'
# Hide RECOVERY from udisks (no auto-mount, no file manager entry)
SUBSYSTEM=="block", ENV{ID_FS_LABEL}=="RECOVERY", ENV{UDISKS_IGNORE}="1"
SUBSYSTEM=="block", ENV{ID_PART_ENTRY_NAME}=="RECOVERY", ENV{UDISKS_IGNORE}="1"

# Kernel-level read-only: writes to this partition fail, even as root
ACTION=="add|change", SUBSYSTEM=="block", ENV{ID_FS_LABEL}=="RECOVERY", RUN+="/usr/sbin/blockdev --setro $env{DEVNAME}"
ACTION=="add|change", SUBSYSTEM=="block", ENV{ID_PART_ENTRY_NAME}=="RECOVERY", RUN+="/usr/sbin/blockdev --setro $env{DEVNAME}"
RULES
chmod 644 "$ROOT/etc/udev/rules.d/99-recovery-protect.rules"

# ---------- 2. no snap ----------
install -d "$ROOT/etc/apt/preferences.d"
cat > "$ROOT/etc/apt/preferences.d/nosnap.pref" <<'PREF'
Package: snapd
Pin: release a=*
Pin-Priority: -10
PREF

if (( LIVE )); then
  apt-get purge -y snapd gnome-software-plugin-snap 2>/dev/null || apt-get purge -y snapd
  apt-mark hold snapd || true
  rm -rf /snap /var/snap /var/lib/snapd /var/cache/snapd
fi

# ---------- 3. Firefox from Mozilla's apt repo ----------
install -d -m 0755 "$ROOT/etc/apt/keyrings" "$ROOT/etc/apt/sources.list.d"

cat > "$ROOT/etc/apt/sources.list.d/mozilla.sources" <<'SRC'
Types: deb
URIs: https://packages.mozilla.org/apt
Suites: mozilla
Components: main
Signed-By: /etc/apt/keyrings/packages.mozilla.org.asc
SRC

cat > "$ROOT/etc/apt/preferences.d/mozilla" <<'PREF'
Package: firefox*
Pin: origin packages.mozilla.org
Pin-Priority: 1001

Package: firefox*
Pin: release o=Ubuntu
Pin-Priority: -1
PREF

if (( LIVE )); then
  apt-get update
  apt-get install -y wget ca-certificates
  wget -q https://packages.mozilla.org/apt/repo-signing-key.gpg -O /etc/apt/keyrings/packages.mozilla.org.asc
  apt-get update
  apt-get remove -y firefox 2>/dev/null || true   # Ubuntu's fake snap-installing stub
  apt-get install -y firefox

  # ---------- 4. Flatpak + Flathub ----------
  apt-get install -y flatpak
  flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo || \
    echo "couldn't add Flathub in the chroot, run: flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo after install"
fi

# ---------- 5. peck (the QuackOS package manager, GURT backend) ----------
install -d "$ROOT/usr/local/bin"
cat > "$ROOT/usr/local/bin/peck" <<'PECKEOF'
#!/usr/bin/env bash
# peck - the QuackOS package manager 🦆  (GURT backend)
# Thin, friendly front-end: every peck command is handed to `gurt`.
# Sources work like gurt's: peck install apt/htop, peck install aur/yay, --repo flatpak, etc.

set -uo pipefail
VERSION="0.2.0"
GURT="${PECK_GURT:-gurt}"
GURT_INSTALL="curl -fsSL https://raw.githubusercontent.com/clrealy/GURT/main/install.sh | sh"

help() {
  cat <<'TXT'
🦆 peck - the QuackOS package manager (powered by GURT)

  peck install <pkg...>    install packages        (aliases: add, get)
  peck remove  <pkg...>    remove packages         (aliases: rm, uninstall)
  peck update              pull the latest recipes (gurt sync)
  peck upgrade             upgrade everything gurt installed
  peck search  <term>      search packages         (alias: find)
  peck info    <pkg>       show package details    (alias: show)
  peck list                what you've installed   (alias: ls)
  peck autoremove          yeet deps nothing needs anymore
  peck run     <app>       start an app (offers to install it first)
  peck de      <name>      desktop environments    (peck de kde)
  peck history | cache | verify | profile | files | owns | why | compare | hogs | notify
                           straight through to the gurt command of the same name
  peck -y ...              say yes to everything
  peck gurt    <args...>   pass anything to gurt as-is
  peck help | version

Pick a source like gurt does:  peck install apt/htop   peck install aur/yay   peck search -a fetch
Set PECK_GURT=/path/to/gurt to use a different gurt binary.
TXT
}

need_gurt() {
  if ! command -v "$GURT" >/dev/null 2>&1; then
    echo "🦆 peck can't find GURT ('$GURT'). Install it first:" >&2
    echo "   $GURT_INSTALL" >&2
    exit 127
  fi
}

# leading -y / --yes goes before the gurt verb (that's where gurt wants it)
PRE=()
while [[ "${1:-}" == "-y" || "${1:-}" == "--yes" ]]; do PRE+=("$1"); shift; done

cmd="${1:-help}"; [[ $# -gt 0 ]] && shift

go() {  # go <gurt-verb> [args...]
  need_gurt
  echo "🦆 pecking: $*" >&2
  exec "$GURT" ${PRE[@]+"${PRE[@]}"} "$@"
}

need_args() {  # need_args <verb> <arg-count>
  if [[ "$2" -lt 1 ]]; then
    echo "peck $1: what should I peck at? (try: peck $1 <name>)" >&2
    exit 2
  fi
}

case "$cmd" in
  install|add|get)        need_args install $#; go install "$@" ;;
  remove|rm|uninstall)    need_args remove $#;  go remove "$@" ;;
  update)                 go sync "$@" ;;
  upgrade)                go upgrade "$@" ;;
  search|find)            need_args search $#;  go search "$@" ;;
  info|show)              need_args info $#;    go info "$@" ;;
  list|ls)                go list "$@" ;;
  de)                     need_args de $#;      go de "$@" ;;
  autoremove|run|history|cache|verify|profile|files|owns|why|compare|hogs|notify|sync|gui)
                          go "$cmd" "$@" ;;
  gurt)                   go "$@" ;;
  version|-v|--version)   echo "peck $VERSION (backend: $GURT)" ;;
  help|-h|--help)         help ;;
  *) echo "peck: unknown command '$cmd' (try: peck help)" >&2; exit 2 ;;
esac
PECKEOF
chmod 755 "$ROOT/usr/local/bin/peck"

# ---------- 6. GURT (the backend peck talks to) ----------
# Official installer: needs bash + git + GNU tar. GURT_WIZARD=0 skips the interactive
# setup wizard (it would hang the Cubic terminal). Failure here won't stop the build.
if (( LIVE )); then
  apt-get install -y bash git curl tar ca-certificates
  if curl -fsSL https://raw.githubusercontent.com/clrealy/GURT/main/install.sh -o /tmp/gurt-install.sh \
     && GURT_WIZARD=0 sh /tmp/gurt-install.sh; then
    echo "GURT installed: $(gurt version 2>/dev/null || echo ok)"
  else
    echo "!! GURT install failed. Finish after boot with:" >&2
    echo "   curl -fsSL https://raw.githubusercontent.com/clrealy/GURT/main/install.sh | sh" >&2
  fi
  rm -f /tmp/gurt-install.sh
fi


echo "done: RECOVERY protected, snap removed, Firefox + Flatpak + peck + GURT installed"
