#!/usr/bin/env bash
# Single source of truth for release-version consistency. Both gates invoke
# THIS script so they cannot drift apart:
#
#   - tag time: .github/workflows/release.yml verify-version passes the tag's
#     version — every file must equal it, or the tag is rejected.
#   - PR time:  the ci.yml `version-drift` job runs it with no argument —
#     drift is caught while the tag is still cheap to re-cut (v0.17.0 never
#     shipped because .SRCINFO lagged and the check only ran at tag time).
#
#   scripts/check-release-versions.sh           mutual agreement of the six
#                                               version locations + CHANGELOG
#   scripts/check-release-versions.sh 1.9.0     additionally require every
#                                               location to equal this version
#
# The six locations (MUTUAL AGREEMENT — kde-plasmoid/package/metadata.json and
# gnome-extension/metadata.json are deliberately NOT among them; both are
# versioned independently of Cargo.toml):
#
#   Cargo.toml  manifest.json  packaging/aur/PKGBUILD  PKGBUILD-bin
#   packaging/aur/.SRCINFO  .SRCINFO-bin
#
# Plus, for the agreed version: a `## [X.Y.Z] — ` CHANGELOG.md section must
# exist, and — when a previous tag exists — no already-published CHANGELOG
# section may have moved (CLAUDE.md release checklist step 2).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required to read manifest.json (install jq and retry)" >&2
  exit 1
fi

# Empty = only mutual agreement (PR gate); set = agreement AND this exact
# version (tag gate). Falls back to Cargo.toml's own version otherwise.
expected="${1:-}"

fail=0
note() { # note <message> — record a failure, keep collecting the rest
  echo "✗ $1" >&2
  fail=1
}

# Each extractor mirrors release.yml's original verify-version greps verbatim,
# but a malformed or missing input now fails with a line naming the file
# instead of a bare grep/jq exit code.
cargo_ver=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2) || {
  echo "error: no '^version' line found in Cargo.toml" >&2
  exit 1
}
if [[ -z "$expected" ]]; then
  expected="$cargo_ver"
fi

manifest_ver=$(jq -r '.version' manifest.json) || {
  echo "error: could not read .version from manifest.json — is it valid JSON?" >&2
  exit 1
}
pkgbuild_ver=$(grep -m1 '^pkgver=' packaging/aur/PKGBUILD | cut -d= -f2) || {
  echo "error: no 'pkgver=' line found in packaging/aur/PKGBUILD" >&2
  exit 1
}
pkgbuild_bin_ver=$(grep -m1 '^pkgver=' packaging/aur/PKGBUILD-bin | cut -d= -f2) || {
  echo "error: no 'pkgver=' line found in packaging/aur/PKGBUILD-bin" >&2
  exit 1
}
srcinfo_ver=$(grep -m1 'pkgver = ' packaging/aur/.SRCINFO | awk '{print $3}') || {
  echo "error: no 'pkgver = ' line found in packaging/aur/.SRCINFO" >&2
  exit 1
}
srcinfo_bin_ver=$(grep -m1 'pkgver = ' packaging/aur/.SRCINFO-bin | awk '{print $3}') || {
  echo "error: no 'pkgver = ' line found in packaging/aur/.SRCINFO-bin" >&2
  exit 1
}

check() { # check <label> <actual>
  if [[ "$2" != "$expected" ]]; then
    note "$1 is '$2' but expected '$expected'"
  fi
}
check "Cargo.toml version" "$cargo_ver"
check "Omarchy plugin version (manifest.json)" "$manifest_ver"
check "PKGBUILD pkgver" "$pkgbuild_ver"
check "PKGBUILD-bin pkgver" "$pkgbuild_bin_ver"
check ".SRCINFO pkgver" "$srcinfo_ver"
check ".SRCINFO-bin pkgver" "$srcinfo_bin_ver"

if ! grep -Eq "^## \[$expected\] — " CHANGELOG.md; then
  note "CHANGELOG.md has no '## [$expected] — ' section"
fi

# No published section may move (CLAUDE.md step 2): diff CHANGELOG against the
# last tag; everything removed except the `[Unreleased]:` compare link is a
# regression. Skipped when the checkout has no tags (fresh clone, tarball).
if prev_tag=$(git describe --tags --abbrev=0 2>/dev/null); then
  drift=$(git diff "$prev_tag" HEAD -- CHANGELOG.md \
    | grep '^-' | grep -v '^---' | grep -v '^-\[Unreleased\]:' || true)
  if [[ -n "$drift" ]]; then
    note "CHANGELOG.md moved published content since $prev_tag:"
    printf '%s\n' "$drift" >&2
  fi
else
  echo "no previous tag found — skipping the no-published-section-moved diff"
fi

if [[ "$fail" -ne 0 ]]; then
  echo "release-version check FAILED" >&2
  exit 1
fi
echo "✓ release versions agree at $expected (6 files + CHANGELOG section)"
