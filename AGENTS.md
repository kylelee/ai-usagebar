# PROJECT KNOWLEDGE BASE

**Generated:** 2026-08-31 · **Commit:** 7220235 · **Branch:** main
**Canonical for release checklist + hard invariants:** `CLAUDE.md` (read it first).

## OVERVIEW

Rust CLI + TUI + four desktop frontends showing AI plan usage/balances across 18
vendors (Claude, Codex, Z.AI, OpenRouter, …). Began as a Rust port of `claudebar`;
per-vendor Waybar modules stay drop-in compatible with it.

## STRUCTURE

```
src/               # single crate ai_usagebar: lib + shared core + 19 vendor modules
├── bin/           # 2 binaries: ai-usagebar-omarchy (widget CLI), -tui (ratatui)
├── widget/        # Waybar surface: cli/run/render/aggregate/pretty (own AGENTS.md)
├── tui/           # ratatui app, panels, settings overlay, context viewer
└── <vendor>/      # anthropic, openai, zai, openrouter, kiro, cursor, … (see src/AGENTS.md)
omarchy/           # PRIMARY frontend: Quattro QML panel (manifest.json at repo root)
gnome-extension/   # GNOME Shell extension (marker-logic.js = pure helpers)
kde-plasmoid/      # KDE Plasma 6 plasmoid (KPackage)
macos/             # Swift menubar app (single file + tests, own CI job)
packaging/aur/     # PKGBUILD + PKGBUILD-bin + .SRCINFOs (AUR; CI auto-publishes)
tests/             # anthropic_e2e (mockito+insta), live (#[ignore]), public_api
docs/              # user-facing reference guides
manifest.json      # Omarchy plugin manifest — entry point omarchy/BarWidget.qml
Makefile           # test = cargo test + desktop-test + plugin-test (use it, not bare cargo)
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add a vendor | `src/<name>/` + `vendor.rs::VendorId` + `usage.rs::*Snapshot` | anatomy in src/AGENTS.md |
| Widget output / Waybar JSON | `src/widget/` (run, render, cli, aggregate) | exit-0 invariant lives here |
| Report JSON (`usage --json`) | `src/report.rs` | `metrics` gauges vs `sections` ordered rows |
| Config fields | `src/config.rs` (3.5k lines) + `src/tui/settings.rs` + `omarchy/SettingsView.qml` | keep all three in sync |
| Accounts / multi-key | `src/account.rs` | per-account cache isolation |
| Cache / atomic writes / flock | `src/cache.rs` | `Cache::at` is the test seam |
| Active-vendor cycle file | `src/active.rs` | KDE plasmoid deliberately never reads it |
| Money formatting | `src/format.rs` (`money`, `usd`) | negative sign outside symbol |
| Sanitizing untrusted text | `src/pango.rs`, `src/display.rs` | sink, not call site |
| Release process | `CLAUDE.md` steps 1–9 | version lives in 4 places (Cargo.toml, manifest.json, both PKGBUILDs) |

## CODE MAP

From LSP (rust-analyzer); cross-file ref counts unmeasured this session — centrality
taken from CLAUDE.md's "single source" invariants.

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| `VendorId` | enum (18) | src/vendor.rs:110 | `slug`/`display_name`/`short_name`/`logo_slug` — THE label source |
| `VendorSnapshot` | enum (18) | src/usage.rs:340 | canonical data model; all `*Snapshot` structs in usage.rs |
| `VendorFetchOutcome` | trait | src/vendor.rs:278 | fetch result → snapshot + stale/last_error/cache_age |
| `RenderOpts` | struct | src/vendor.rs:327 | format/icon/pace flags from CLI |
| `Cache::at` | fn | src/cache.rs | path-injecting constructor (hermetic tests) |
| `SectionBuilder::push_metric` | method | src/report.rs | metric rows must carry absolute reset |
| lib.rs | root | src/lib.rs:11-55 | declares all 43 modules |

## CONVENTIONS

- Rust 2024, no cargo features. Deps pinned in Cargo.toml (tokio, reqwest, ratatui,
  rusqlite, toml_edit, security-framework/win-sys per-OS).
- Frontends stay THIN: fetching/credentials/names/money/metrics in Rust; frontend only
  projects the report model. Never add a provider-name table to a frontend.
- Report fields are additive — existing JSON consumers must keep working.
- Commit messages: `type(scope): summary` (conventional commits, some zh-CN bodies OK).

## ANTI-PATTERNS (THIS PROJECT)

- `format!("${v:.2}")` — balances can be negative; use `format::{money,usd}`.
- Widget exiting non-zero — Waybar hides it; always fall back to `⚠` JSON.
- Non-atomic cache writes (multi-monitor Waybar races).
- Bare `.display()` on untrusted paths reaching println/eprintln — sanitize at sink.
- Tests touching real `$HOME`/`$XDG` or ambient env — AUR `check()` runs `cargo test`
  on user machines (full seam list in src/AGENTS.md).
- Force-moving a pushed tag; committing real keys; `cat`ing credential files.
- Restating CLAUDE.md's release steps elsewhere — CLAUDE.md is canonical.

## UNIQUE STYLES

- Pango bordered-box tooltip shared by every vendor (`src/tooltip.rs`).
- Per-figure range colors: each window figure colored by ITS own remaining band.
- Claudebar flag/placeholder compatibility (`--format`, `--tooltip-format`).
- KDE `KPlugin.Version` (metadata.json) and GNOME `metadata.json` version
  independently of Cargo.toml — bump when that frontend dir changes.

## COMMANDS

```bash
make test                 # cargo test + gnome/kde node suites + omarchy plugin suite
make clippy               # cargo clippy --all-targets -- -D warnings
cargo machete             # unused-dep gate
omarchy plugin validate . # manifest + entry points (before any release tag)
make smoke                # live API tests: cargo test --test live -- --ignored
make desktop-test         # node gnome-extension + kde-plasmoid logic tests only
make plugin-test          # node omarchy/model.test.mjs only
make qml-lint qml-test    # Qt6-only gates (not in CI; need qt6 tools)
make mjs-probe            # plasmoidviewer V4-engine gate (needs Plasma session)
make install PREFIX=/usr  # + install-plasmoid for KDE (KPackage only scans XDG_DATA_DIRS)
```

CI (`.github/workflows/ci.yml`): ubuntu (fmt+clippy+test+node suites), rust 1.88 MSRV
check, nix flake, macOS (clippy+test+swiftc+`./macos/run-tests.sh`), Windows
(clippy+test+node suites). Release is tag-driven (`release.yml`) with automated AUR publish.

## NOTES

- Two `manifest.json`s: repo root (Omarchy plugin, entry `omarchy/BarWidget.qml`) and
  `kde-plasmoid/package/metadata.json` (KPlugin) — different schemas, both versioned.
- Config template auto-dropped at `~/.config/ai-usagebar-omarchy/config.toml` (mode 600,
  never rewritten); `[vendor] enabled = true` gates most vendors.
- `.SRCINFO` regeneration must happen BEFORE tagging or the release job rejects the tag.
- Subdirectory knowledge: `src/AGENTS.md`, `src/tui/AGENTS.md`, `src/widget/AGENTS.md`,
  `omarchy/AGENTS.md`, `kde-plasmoid/AGENTS.md`, `gnome-extension/AGENTS.md`,
  `macos/AGENTS.md`.
