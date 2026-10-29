# omarchy/ — PRIMARY frontend (Quattro QML plugin)

Native Omarchy panel: bar tiles per account, popup detail, settings form.
The repo-root `manifest.json` (NOT an omarchy/ file) is the plugin manifest —
entry point `omarchy/BarWidget.qml`; validate with `omarchy plugin validate .`.

## STRUCTURE

```
BarWidget.qml (215)  # manifest entry: bar tile + click/wheel routing
Panel.qml (886)      # popup: per-account detail, per-figure colors, countdowns
SettingsView.qml(996)# native settings form (account cards, add/delete, Apply)
Model.js (918)       # PURE adapter: `usage --json` report → QML model. No fetching.
model.test.mjs (1058)# node contract tests for Model.js — `make plugin-test`
logos/               # per-vendor icons
```

## WHERE TO LOOK

| Task | Do |
|------|----|
| Bar tile look | `BarWidget.qml` — per-figure range colors, per-countdown clock bands, account tag stays neutral |
| Panel rows/sections | `Panel.qml` — projects `sections` ordered rows from the report |
| Add settings UI | `SettingsView.qml` + keep `src/config.rs` + `src/tui/settings.rs` in sync |
| Report field consumption | `Model.js` only — QML never parses vendor JSON itself |

## CONVENTIONS

- THIN adapter: provider names/short codes/money/metrics come from the report
  (`display_name`, `short_name`, severity). Never add a provider table here.
- Model.js is pure JS (node-testable, no Qt imports) — keep it that way so
  `model.test.mjs` stays the contract gate that runs in every CI job.
- API keys typed in SettingsView are sent to the Rust settings command over STDIN —
  never as command-line arguments (visible in `ps`). Stored values are never loaded
  back into the long-lived shell process: blank = unchanged, clear = explicit remove.
- Follows the shell theme (`~/.config/omarchy/current/theme/colors.toml`); color
  bands green ≥50% / yellow <50% / orange <20% / red <5% per figure.
- Poll default 60s (manifest `refreshIntervalSec`, min 30) — Claude/Codex are
  internally throttled to a 300s cache TTL by Rust; don't add per-vendor timers here.
- Exhausted accounts leave the bar until room returns; tabs/popup keep them — that's
  intended, not a bug.

## ANTI-PATTERNS

- Don't read `~/.config/ai-usagebar-omarchy/` directly from QML — all IO through
  the Rust binaries.
- Don't duplicate `plasmoid-logic`/`marker-logic` helpers here; each frontend owns
  its own pure adapter, tested by its own node suite.
