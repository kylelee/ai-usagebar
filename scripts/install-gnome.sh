#!/usr/bin/env bash
# Compile + install the GNOME Shell extension from THIS checkout (repo-root
# entry point; gnome-extension/install.sh does the same from inside the
# extension dir).
#
#   ./scripts/install-gnome.sh
#
#   1. install the CLI binaries — ./scripts/install-bin.sh compiles
#      (./scripts/build.sh) and copies the executables onto a writable
#      PATH dir (BIN_DIR override)
#   2. glib-compile-schemas gnome-extension/schemas/
#      glib-compile-schemas NEVER exits non-zero on schema errors (it
#      prints to stderr and ignores the file), so stderr is checked —
#      a broken schema must fail the install, not ship silently.
#   3. COPY gnome-extension/ into
#      ~/.local/share/gnome-shell/extensions/<uuid>  (replaces any
#      previous install of the same uuid)
#      A real copy, never a symlink: the installed extension keeps
#      working after this checkout moves, is renamed, or is deleted.
#      Re-run this script after code changes to redeploy.
#   4. gnome-extensions enable <uuid>  (best-effort: the shell only scans
#      local extensions at login, so before the first reload this prints
#      the manual next steps instead of failing)
#
# The extension executes the `ai-usagebar-omarchy` binary from the session
# PATH (settings bridge + usage report) — step 1 installs it.
#
# Logs: journalctl -f -o cat /usr/bin/gnome-shell
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
UUID="ai-usagebar-omarchy@kylelee.github.io"
SRC="$REPO/gnome-extension"
DEST="$HOME/.local/share/gnome-shell/extensions/$UUID"

# 1. The binaries the extension spawns.
"$REPO/scripts/install-bin.sh"

echo "› Compiling schema…"
stderr=$(glib-compile-schemas "$SRC/schemas" 2>&1)
if [[ -n $stderr ]]; then
    echo "glib-compile-schemas failed:" >&2
    echo "$stderr" >&2
    exit 1
fi

echo "› Copying $SRC → $DEST"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
cp -a "$SRC" "$DEST"

if gnome-extensions enable "$UUID" 2>/dev/null; then
    echo "✓ Installed and enabled."
else
    echo "✓ Installed (copied)."
    echo
    echo "Next:"
    echo "  1. Reload GNOME Shell:"
    echo "       • X11     → Alt+F2, type 'r', Enter"
    echo "       • Wayland → log out and back in"
    echo "  2. Enable it:"
    echo "       gnome-extensions enable $UUID"
    echo "  3. Settings (accounts, tiles, colors):"
    echo "       gnome-extensions prefs $UUID"
fi
