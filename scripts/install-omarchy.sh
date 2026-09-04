#!/usr/bin/env bash
# Full local-testing deploy from THIS checkout for the OMArchY frontend:
# build + install the CLI binaries, then reset + reinstall the Omarchy
# Quattro plugin.
#
#   install-bin.sh                 compile (./scripts/build.sh) + copy the
#                                  binaries onto a writable PATH dir (BIN_DIR
#                                  override) — a real copy, never a link
#                                  into target/, which `cargo clean` would
#                                  silently uninstall
#   omarchy plugin remove         drop the current plugin install (reset)
#   omarchy plugin add <repo>     git-clone this repo into the plugins dir
#   overlay working tree          a git clone only carries COMMITTED
#                                 state; uncommitted local edits deploy
#                                 by copying plugin files over the clone
#                                 (manifest.json + omarchy/)
#
# The plugin is only the display frontend; it executes the
# `ai-usagebar-omarchy` binary from the session PATH — hence step 0.
#
# The script restarts the shell at the end: the remove+add cycle deletes and
# re-creates the plugin directory, which orphans the shell's file watchers —
# in-memory QML keeps running the OLD code even though every file on disk is
# new. `omarchy restart shell` is the reliable reload (learned the hard way:
# a "successful" deploy that the bar never showed).
#
# Env: BIN_DIR=…      passed through to install-bin.sh (where the binaries
#                     are copied; default: first writable dir on PATH)
#      SKIP_BINARY=1  skip the build+install (plugin-only deploy)
#      SKIP_RESTART=1 skip the shell restart
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
PLUGIN_ID=$(jq -r '.id' "$REPO/manifest.json")
PLUGINS_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins"
TARGET="$PLUGINS_DIR/$PLUGIN_ID"

if ! command -v omarchy >/dev/null 2>&1; then
  echo "omarchy CLI not found — this script is for an Omarchy 4 machine." >&2
  exit 1
fi

# 0. Compile + install the binaries the plugin executes
#    (./scripts/install-bin.sh owns build, copy, and destination policy).
if [[ ${SKIP_BINARY:-0} != 1 ]]; then
  "$REPO/scripts/install-bin.sh"
fi

# 1. Reset: drop the current install (also fine when none exists yet).
omarchy plugin remove "$PLUGIN_ID" --yes >/dev/null 2>&1 || true

# 2. Install from this checkout. `plugin add` git-clones its argument, so a
#    local path works and the installed copy's origin points here — pulling
#    this repo's committed HEAD.
omarchy plugin add "$REPO" --enable --yes

# 3. Overlay the working tree so uncommitted edits deploy too. Walk with
#    RELATIVE paths (cd into the repo first, same trick as the Makefile's
#    install-plasmoid): an absolute {} would nest the whole source path
#    under $TARGET instead of mirroring omarchy/.
install -Dm644 -- "$REPO/manifest.json" "$TARGET/manifest.json"
(cd "$REPO" && find omarchy -type f -exec install -Dm644 -- {} "$TARGET/{}" \;)

# 4. Prove what landed: the binary the panel will execute, the manifest +
#    entry points, and the JS contract tests against the INSTALLED copy.
command -v ai-usagebar-omarchy
ai-usagebar-omarchy --version
omarchy plugin validate "$TARGET"
node "$TARGET/omarchy/model.test.mjs"

# 5. Reload the running shell so it actually picks the new files up.
if [[ ${SKIP_RESTART:-0} != 1 ]]; then
  omarchy restart shell
fi

echo "deployed: binary → ${BIN_DIR:-<skipped>}, plugin → $TARGET"
