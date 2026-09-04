# macos/ — Swift menubar app

Single-file SwiftUI menubar frontend + single-file test target. Own CI lane
(macos-latest: `swiftc -O -parse-as-library` compile + `./macos/run-tests.sh`).
Parent: `../AGENTS.md`.

## STRUCTURE

```
ai-usagebar-menubar.swift   # the whole app (menubar item, popover, refresh timer)
ai-usagebar-tests.swift     # pure-logic unit tests (XCTest-style, run via run-tests.sh)
build.sh / install-agent.sh # local build / LaunchAgent install helpers
run-tests.sh                # CI test entry (compiles tests against the app file)
INSTALL.md / README.md
```

## CONVENTIONS

- Consumes the same `ai-usagebar-omarchy usage --json` report as every other
  frontend — same thin-adapter rule, same additive-fields contract.
- Claude OAuth on macOS may live in the login Keychain (`Claude Code-credentials`
  item, per-`CLAUDE_CONFIG_DIR` scoped items `…-<hash>`); reading goes through the
  Rust side (`src/anthropic/keychain.rs`), never from Swift directly.
- Testable logic stays in pure functions inside the app file so `ai-usagebar-tests.swift`
  can compile against them without a bundle/app host.
- `swiftc -parse-as-library` is the CI compile gate — keep the file parseable as a
  library (no top-level executable statements outside `main`-style entry).
- Bump `appVersion` in `ai-usagebar-menubar.swift` when anything under `macos/`
  changes, keeping it equal to Cargo.toml's `version` (mirrors the KDE/GNOME per-frontend version convention).

## ANTI-PATTERNS

- Don't add Xcode project files or SPM manifests — the single-file + shell-script
  build is deliberate (CI has only swiftc).
- Don't read credentials or config files directly from Swift; shell out to the
  Rust binaries so sanitizing/caching semantics stay in one place.
