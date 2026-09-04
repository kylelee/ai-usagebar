#!/usr/bin/env bash
# Mechanical half of the CLAUDE.md release checklist: steps 1/3/4/5 — the six
# version locations plus the CHANGELOG [Unreleased] compare link. Invoked as
#   make release-bump V=X.Y.Z
#
# Deliberately does NOT: write the release's own CHANGELOG section (step 2 —
# human judgment), commit, tag, or push anything. It ends with
# `make release-check`, which reports the missing `## [X.Y.Z] — ` section
# until step 2 is done — that failure is the built-in reminder, not a bug.
#
# .SRCINFO portability: the pkgver sed below works on any machine, but on
# Arch the AUTHORITATIVE regeneration is `makepkg --printsrcinfo` (the
# scratch-dir procedure in CLAUDE.md step 5) — prefer it when available.
set -euo pipefail

usage() {
  echo "usage: make release-bump V=X.Y.Z   (or: scripts/release-bump.sh X.Y.Z)" >&2
}

die() {
  echo "error: $1" >&2
  exit 1
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

V="${1:-}"
[[ -n "$V" ]] || { usage; die "no version given"; }

# Semver core with an optional prerelease suffix (e.g. 2.0.0, 0.0.0-test).
# Validated BEFORE any edit so malformed input changes nothing.
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]] \
  || die "'$V' is not X.Y.Z (optionally with a -prerelease suffix); nothing was changed"

command -v jq >/dev/null 2>&1 || die "jq is required (manifest.json version)"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git checkout"

dirty=$(git status --porcelain)
[[ -z "$dirty" ]] || {
  echo "refusing to bump: the working tree is dirty — commit or stash first." >&2
  echo "release-bump edits six files in place and must stay revertible by git:" >&2
  printf '%s\n' "$dirty" >&2
  exit 1
}

old=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
[[ -n "$old" ]] || die "no '^version' line found in Cargo.toml"
[[ "$old" != "$V" ]] || die "version is already $V"

# Step 1 — Cargo.toml (single top-level `version` line) + manifest.json.
sed -i "s/^version = \".*\"/version = \"$V\"/" Cargo.toml
tmp=$(mktemp)
jq --arg v "$V" '.version = $v' manifest.json > "$tmp" && mv "$tmp" manifest.json

# Steps 3–4 — both PKGBUILDs: pkgver, pkgrel=1, sha256s back to 'SKIP'
# (the release workflow's publish-aur job pins the real hashes later).
sed -i -e "s/^pkgver=.*/pkgver=$V/" -e "s/^pkgrel=.*/pkgrel=1/" \
       -e "s/^sha256sums=.*/sha256sums=('SKIP')/" packaging/aur/PKGBUILD
sed -i -e "s/^pkgver=.*/pkgver=$V/" -e "s/^pkgrel=.*/pkgrel=1/" \
       -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('SKIP')/" \
       -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('SKIP')/" \
       packaging/aur/PKGBUILD-bin

# Step 5 (portable form) — .SRCINFOs carry one `pkgver = ` line each; the
# `provides = ai-usagebar=$pkgver` rows keep the literal variable untouched.
sed -i "s/^\tpkgver = .*/\tpkgver = $V/" packaging/aur/.SRCINFO
sed -i "s/^\tpkgver = .*/\tpkgver = $V/" packaging/aur/.SRCINFO-bin

# Step 2's link half — [Unreleased] now compares against the tag being cut.
# The `## [X.Y.Z] — date` section itself is deliberately NOT added here.
sed -i "s|^\[Unreleased\]: .*/compare/v$old\.\.\.HEAD$|[Unreleased]: https://github.com/KyleLee/ai-usagebar-omarchy/compare/v$V...HEAD|" \
  CHANGELOG.md
grep -q "^\[Unreleased\]: .*/compare/v$V\.\.\.HEAD$" CHANGELOG.md \
  || die "CHANGELOG.md has no '[Unreleased]: …/compare/v$old...HEAD' link to update (step 2 moved it?) — the six version files are already bumped; fix the link by hand and continue"

echo "bumped $old → $V in Cargo.toml, manifest.json, both PKGBUILDs (pkgrel=1, sha256 'SKIP'), both .SRCINFOs, and the [Unreleased] compare link"
echo "remaining manual step: CLAUDE.md step 2 — add the '## [$V] — YYYY-MM-DD' section (release-check below stays red until it exists)"
echo "nothing was committed, tagged, or pushed — review with: git diff"

make release-check
