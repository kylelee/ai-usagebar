# kde-plasmoid/ — KDE Plasma 6 plasmoid (KPackage)

`package/` is the shippable tree; everything else is scaffolding/tests.
Parent: `../AGENTS.md`.

## STRUCTURE

```
package/metadata.json           # KPlugin manifest — Id MUST equal Makefile PLASMOID_ID
package/contents/code/plasmoid-logic.mjs (538)  # PURE adapter: report JSON → display model
package/contents/ui/            # main.qml + widgets + config/*.qml (KConfigXT)
package/contents/config/main.xml
plasmoid-logic.test.mjs         # node tests — part of `make desktop-test` (Windows CI too)
probe/ + qmltests/              # runtime gates (see below)
install.sh                      # user install helper; Makefile install-plasmoid is canonical
```

## GATES (each catches a different class of bug)

| Command | Catches | Needs |
|---------|---------|-------|
| `node plasmoid-logic.test.mjs` | adapter logic | node only |
| `make qml-lint` | QML syntax (Qt6 qmllint, `--unqualified disable` for i18n) | qt6 tools |
| `make qml-test` | rendered geometry/colors — UsageBar.qml ONLY; main.qml can't run hostless (Plasmoid attached property is injected at runtime) | qt6 + Kirigami, offscreen |
| `make mjs-probe` | V4-vs-V8 engine diffs (Node accepts `catch {` and `\p{...}`, QML doesn't — the latter SILENTLY) | plasma-sdk + live session |

## CONVENTIONS

- `KPlugin.Version` in `package/metadata.json` is bumped INDEPENDENTLY of Cargo.toml
  whenever this directory changes in a release.
- This plasmoid DELIBERATELY never reads/writes the active-vendor cycle file
  (`src/active.rs`) — per-applet vendor selection via KConfigXT config instead.
  The GNOME extension + Waybar share the cycle file; don't "unify" this.
- `plasmoid-logic.mjs` is its own pure module — NOT a copy of gnome's
  `marker-logic.js` (different data shape, different tests). Keep both pure so
  their node suites run on any CI OS.
- V4 engine constraints for ALL .mjs here: no optional-catch-binding, no
  `\p{...}` regex, stick to ES2020-level syntax Node already accepts.
- Install: KPackage only scans `$XDG_DATA_DIRS` → system installs need
  `make install install-plasmoid PREFIX=/usr` (not /usr/local).
- mjs-probe verdict comes from probe markers (`MJS PROBE OK` + `EXEC DONE`, no
  FAIL lines) — grep exit codes and plasmoidviewer timeouts are meaningless here.
  It's a real gate: never end it in `|| true`.

## ANTI-PATTERNS

- Don't test main.qml with qmltestrunner — the applet host injects `Plasmoid.*`;
  KDE documents no mock. Extend the probe instead.
- Don't hardcode provider names/short codes in QML — take them from the report.
