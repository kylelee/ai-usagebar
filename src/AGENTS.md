# src/ — ai_usagebar crate

Single crate: shared core (20 root files) + 19 vendor modules + `context/` + `widget/`
+ `tui/` + 2 bins in `../src/bin/` (widget CLI, TUI). Repo map + invariants:
`../AGENTS.md`, `../../CLAUDE.md`. lib.rs declares all 43 modules.

## STRUCTURE

```
vendor.rs / usage.rs    # VendorId enum + all *Snapshot structs + VendorSnapshot enum
config.rs (3.5k)        # all TOML fields, account sections, template drop
cache.rs                # atomic writes + flock; Cache::at = path-injecting test seam
report.rs               # usage --json: metrics (gauges) vs sections (ordered rows)
account.rs              # named accounts, per-account cache isolation
active.rs               # active-vendor cycle file (KDE never reads it)
format.rs               # money/usd — negative sign outside the symbol
display.rs / pango.rs   # untrusted-text sanitizing AT THE SINK
tooltip.rs / theme.rs   # shared bordered Pango tooltip; Omarchy theme merge
widget/                 # waybar surface: cli, run, render, aggregate, pretty
tui/                    # ratatui app (own AGENTS.md)
context/                # Claude Code session reader for the TUI context overlay
<vendor>/ ×19           # anthropic, anthropic_api, openai, openai_api, zai,
                        # openrouter, deepseek, kimi, kilo, novita, moonshot,
                        # grok, supergrok, minimax, antigravity, cursor, kiro,
                        # opencode_go, claude_desktop
```

## VENDOR MODULE ANATOMY (uniform)

```
src/<name>/
├── mod.rs      # public surface: fetch() -> VendorOutcome
├── types.rs    # request/response DTOs (serde)
├── fetch.rs    # HTTP + parsing → <Name>Snapshot
└── vendor.rs   # VendorId impl glue / render mapping
```

Optional files appear only when the source demands them: `db.rs` (kiro `data.sqlite3`,
cursor `state.vscdb` — both read-only), `oauth.rs` (kiro refresh, mirrors
`openai::oauth::REFRESH_BUFFER_SECS`), `keychain.rs`/`creds.rs`/`desktop_creds.rs`/
`cli_account.rs` (anthropic macOS + multi-account), `merge.rs` (claude_desktop).

## WHERE TO LOOK

| Task | Do |
|------|----|
| Add vendor | new `src/<name>/` (anatomy above) + `VendorId` variant + `<Name>Snapshot` in usage.rs + `VendorSnapshot` variant + config section + lib.rs module |
| Add snapshot field | usage.rs struct → report.rs section/metric → widget/tui render; report fields are ADDITIVE |
| Change waybar output | `src/widget/render.rs`; CLI flags in `widget/cli.rs` (claudebar-compatible) |
| Add config field | `config.rs` struct + template text + `tui/settings.rs` + `omarchy/SettingsView.qml` — keep in sync |
| Understand a vendor's endpoint | `<vendor>/fetch.rs` top-of-file comment + `docs/vendor-endpoints.md` |

## COMPLEXITY HOTSPOTS (files >1500 lines)

`config.rs` 3581 · `tui/settings.rs` 3103 · `antigravity/fetch.rs` 2032 ·
`tui/panels.rs` 2015 · `claude_desktop/merge.rs` 1894 · `claude_desktop/mod.rs` 1606 ·
`tui/app.rs` 1578. Read before editing; `antigravity/fetch.rs` is the least-typical
vendor (local loopback RPC, `always arrives last and therefore always wins` event merge).

## CONVENTIONS

- Hermetic tests ONLY — full seam list (AUR `check()` runs `cargo test` on user boxes):
  `Cache::at`, `read_from`/`write_to`/`cycle_at` (active.rs), `creds::read_from`,
  `Theme::merged_with_omarchy_file`, `Cli::resolve_vendor_with`, `App::with_theme`,
  `fetch_snapshot_at` (per-vendor fetch seams), `candidate_bases_with` (claude_desktop).
- No test touches real `$HOME`/`$XDG` dirs, ambient env vars, `/proc`, or lsof.
- No absolute wall-clock assertions — pin `now` for snapshot tests; relative
  `Utc::now()` offsets are acceptable (the strict no-wall-clock rule applies
  to antigravity).
- Snapshot equality: hand-written `PartialEq` (serde-derived Eq mismatches float rounding).
- Kiro `pct()` rounds the credit ratio; Kimi `pct()` is exact above f64 precision —
  don't "fix" one to match the other (unit tests pin both).
- `cache.rs` paths "never move (issue #14, back-compat rule 2)".
- Mocked e2e: `tests/anthropic_e2e.rs` (mockito + insta snapshots in `tests/snapshots/`).
  Live API checks: `tests/live.rs`, all `#[ignore]`, run via `make smoke`.
- Vendor-specific logic stays in the vendor module; shared helpers go in core files.
