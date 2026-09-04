#!/usr/bin/env bash
# Compile and copy the Rust executables onto a user-writable directory on
# the session PATH — a real copy, never a link into target/, which `cargo
# clean` would silently uninstall.
#
#   ./scripts/install-bin.sh              build, then copy both binaries
#   BIN_DIR=… ./scripts/install-bin.sh    force one destination
#
# Both frontend installers reference this script (install-omarchy.sh,
# install-gnome.sh): the plugin and the extension spawn
# `ai-usagebar-omarchy` from the session PATH, so a deploy that skips the
# binaries ships a broken surface. It is also runnable on its own — it
# compiles through ./scripts/build.sh (incremental: a warm target/ makes
# the build a no-op) before copying.
#
# Destination policy (BIN_DIR unset): the first WRITABLE dir of
# ~/.local/bin, ~/.cargo/bin, mise's shims. On read-only-$HOME mounts
# (btrfs subvolumes) the usual suspects can be unwritable, which leaves
# mise's shims — writable, on the session PATH, and not tool-managed
# (unlike mise/installs/<tool>/bin, which a tool upgrade wipes).
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
BINS=(ai-usagebar-omarchy ai-usagebar-omarchy-tui)

# Compile first — build.sh fails loudly when a binary does not land in
# target/release, so its exit status is the copy's precondition.
"$REPO/scripts/build.sh"

MISE_SHIMS="${XDG_DATA_HOME:-$HOME/.local/share}/mise/shims"
writable_bin_dir() {
  local dir
  for dir in "$HOME/.local/bin" "$HOME/.cargo/bin" "$MISE_SHIMS"; do
    [[ -d $dir && -w $dir ]] && { printf '%s\n' "$dir"; return; }
  done
  echo "no writable binary dir found (tried ~/.local/bin, ~/.cargo/bin, mise shims)" >&2
  echo "set one explicitly: BIN_DIR=… $0" >&2
  return 1
}

BIN_DIR="${BIN_DIR:-$(writable_bin_dir)}"

# Remove any stale symlink that once pointed a shim straight into
# target/release — through it, `cargo clean` (or a stale build) silently
# changed or removed the live binary.
for dir in "$MISE_SHIMS" "$BIN_DIR"; do
  for bin in "${BINS[@]}"; do
    if [[ -L "$dir/$bin" ]] &&
       [[ $(readlink "$dir/$bin") == "$REPO"/target/* ]]; then
      rm -- "$dir/$bin"
      echo "removed stale target/ link: $dir/$bin"
    fi
  done
done

for bin in "${BINS[@]}"; do
  install -Dm755 -- "$REPO/target/release/$bin" "$BIN_DIR/$bin"
done
echo "binaries copied to $BIN_DIR"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: $BIN_DIR is not on this shell's PATH — make sure it is on the session PATH" >&2 ;;
esac
