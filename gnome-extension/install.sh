#!/usr/bin/env bash
# Install for the AI Usage Bar GNOME Shell extension.
# Copies this folder into ~/.local/share/gnome-shell/extensions and
# compiles the GSettings schema. Run, then reload the shell. A real copy,
# not a symlink: the installed extension keeps working after this
# checkout moves or is deleted — re-run the script to redeploy changes.
set -euo pipefail

UUID="ai-usagebar-omarchy@kylelee.github.io"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$HOME/.local/share/gnome-shell/extensions/$UUID"

echo "› Compiling schema…"
glib-compile-schemas "$SRC/schemas"

echo "› Copying $SRC → $DEST"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
cp -a "$SRC" "$DEST"

echo
echo "✓ Installed (copied)."
echo
echo "Next:"
echo "  1. Reload GNOME Shell:"
echo "       • X11  → Alt+F2, type 'r', Enter"
echo "       • Wayland → log out and back in"
echo "  2. Enable it:"
echo "       gnome-extensions enable $UUID"
echo "  3. Settings (accounts, tiles, colors):"
echo "       gnome-extensions prefs $UUID"
echo
echo "Logs:  journalctl -f -o cat /usr/bin/gnome-shell"
