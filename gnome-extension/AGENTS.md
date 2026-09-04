# gnome-extension/ — GNOME Shell extension

Thin shell around the Rust report; pure logic lives in one node-testable file.
Parent: `../AGENTS.md`.

## STRUCTURE

```
extension.js (887)           # panel button, tiles, per-key tab popup, refresh loop; shell API glue only
report-model.js (1024)       # PURE adapter over the usage report (ESM port of omarchy/Model.js)
report-model.test.mjs (769)  # node tests — part of `make desktop-test` (Windows CI too)
subprocess-utils.js (158)    # shared Gio/GLib subprocess helpers (shell + prefs processes)
prefs.js (1062)              # GTK4 prefs dialog (accounts + display pages)
schemas/                     # GSettings schemas
install.sh / metadata.json / stylesheet.css / package.json
```

## WHERE TO LOOK

| Task | Do |
|------|----|
| Display logic | `report-model.js` (pure, tested) — never inline it into extension.js |
| Shell integration (panel/menu) | `extension.js` |
| Settings UI | `prefs.js` + matching schema in `schemas/` |
| Version bump | `metadata.json` "version" — independent of Cargo.toml, bump when this dir changes in a release |

## CONVENTIONS

- Logic goes in `report-model.js` as pure functions with node tests; `extension.js`
  stays shell glue so CI can test the logic on any OS (no GNOME required).
- Vendor selection is per-extension GSettings (`vendor` key, type "s" — any
  slug): the extension passes `--vendor` explicitly (extension.js) and never
  reads or writes Waybar's active-vendor state file — like the KDE plasmoid,
  which deliberately stays out of it too. Only Waybar's own scroll-cycle
  commands use that file.
- Same thin-adapter rule as every frontend: names/money/metrics come from the
  report JSON; no provider tables here.
- Vendor rendering differences are data-driven from the report's own fields, not
  branched on vendor id in JS.

## ANTI-PATTERNS

- Don't import KDE's `plasmoid-logic.mjs` or omarchy's `Model.js` — each frontend
  owns its adapter and its test file.
- Don't add npm dev-dependencies; the node suites must run with bare `node`.
