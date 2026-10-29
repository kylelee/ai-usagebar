#!/usr/bin/env bash
# Install for the AI Usage Bar GNOME Shell extension.
# Copies this folder into ~/.local/share/gnome-shell/extensions and
# compiles the GSettings schema. Run, then reload the shell. A real copy,
# not a symlink: the installed extension keeps working after this
# checkout moves or is deleted — re-run the script to redeploy changes.
set -euo pipefail

UUID="ai-usagebar@kylelee.github.io"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$HOME/.local/share/gnome-shell/extensions/$UUID"

# The uuid was renamed from ai-usagebar-omarchy@… — retire any install left
# under the old id so the shell never loads two copies of the extension.
OLD_UUID="ai-usagebar-omarchy@kylelee.github.io"
OLD_DEST="$HOME/.local/share/gnome-shell/extensions/$OLD_UUID"
if [[ -d $OLD_DEST ]]; then
    gnome-extensions disable "$OLD_UUID" 2>/dev/null || true
    rm -rf "$OLD_DEST"
    echo "› Removed old install $OLD_UUID"
fi

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
