# src/widget/ — Waybar widget surface

Body of the `ai-usagebar-omarchy` binary (its `main` in `src/bin/` is 37 lines
of arg-parse + hand-off). Parent: `../AGENTS.md`; exit-0 invariant: `../../CLAUDE.md`.

## STRUCTURE

```
run.rs (1186)      # ALWAYS-exit-0 orchestrator: dispatch, fetch, output mode;
                   # cycle/watch short-circuits; fallback() wraps every error in ⚠ JSON
cli.rs (743)       # clap Parser — claudebar-compatible flags + --pretty/--watch/--json
render.rs (786)    # ANTHROPIC bar text + bordered tooltip → WaybarOutput
                   # (every other vendor renders in its own <vendor>/vendor.rs)
aggregate.rs (691) # --vendor all: one tile per enabled account + combined tooltip;
                   # pure fns over tui::app::{TabId,TabState} + tui::panels projection
pretty.rs (200)    # --pretty/--watch: Pango <span> → ANSI 24-bit translator
```

## WHERE TO LOOK

| Task | Do |
|------|----|
| New CLI flag | `cli.rs` — keep claudebar names/defaults (`--format`, `--tooltip-format`, `--icon`, `--pace-tolerance`, `--color-*`) |
| Output-mode pick | piped stdout → JSON, TTY → pretty (auto); `--json` forces JSON |
| Scroll cycling | `run.rs::run_cycle` — active-vendor bump + SIGRTMIN+13; ignores `--vendor` on purpose |
| `--vendor all` tiling | `aggregate.rs` — tile = account tag + per-window figures; hover = combined report |
| Placeholder set | resolved via `format::{placeholders, substitute}`; reference `docs/format-placeholders.md` |

## CONVENTIONS

- `run()` returns `i32`, never `Err` — every failure path lands in `fallback()`'s ⚠
  JSON. Pinned tests: wraps credentials errors, reports broken config, escapes Pango,
  keeps JSON valid, never leaks auth response bodies.
- Cycle signals waybar ONLY after a successful persist — a failed write must not
  trigger a redraw of the never-changed vendor (scroll would look swallowed).
- Aggregate fetches SEQUENTIALLY on purpose: per-vendor cache locks + upstream rate
  limits. Don't parallelize.
- Rendering fns stay pure over `RenderInput` / `(TabId, TabState)`; all I/O in run.rs.
- Aggregate reuses the TUI's tab/section projection (`tui::app`, `tui::panels`) —
  change window figures there, not per-frontend.

## ANTI-PATTERNS

- Don't print on an error path that bypasses `fallback()` — exit != 0 hides the
  Waybar module.
- Don't build vendor-specific format tables here; placeholders + per-vendor default
  formats already cover it.
