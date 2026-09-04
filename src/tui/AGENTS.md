# src/tui/ — ratatui frontend

`ai-usagebar-omarchy-tui` binary body. Vendor-agnostic shell: it renders
`VendorSnapshot`s, never fetches them itself. Parent: `../AGENTS.md`.

## STRUCTURE

```
app.rs (1578)    # event loop, 60s refresh, tab state, stale ↻ marker, key handling
panels.rs (2015) # per-vendor gauge panels (session/weekly/balances/credits blocks)
settings.rs(3103)# Settings overlay: primary vendor + API keys → config.toml
view.rs          # layout composition (sidebar/navbar/no-box navigation modes)
style.rs         # theme colors; App::with_theme is the test seam
context.rs       # optional Claude Code context overlay ([context] enabled = true)
```

## WHERE TO LOOK

| Task | Do |
|------|----|
| New key binding | `app.rs` key match + hint footer |
| New panel row | `panels.rs` — per-vendor fns, follow existing gauge/block style |
| Save a setting | `settings.rs` ONLY — toml_edit round-trip, mode 600, then SIGRTMIN+13 nudge |
| Context overlay | `context.rs` — layouts full/split/bottom (`v` cycles) |

## CONVENTIONS

- Settings persistence MUST go through `toml_edit` (preserves comments/unrelated
  fields). Never `toml`+rewrite — that destroys the user's commented config.
- Saving a non-empty key sets that vendor's `enabled = true`; clearing it removes
  the inline key. Env var still wins at runtime (resolution order in README).
- Key fields masked; `Ctrl-V` toggles reveal; `Ctrl-S` saves.
- context.rs reads only bounded tails of the 100 most-recent Claude Code sessions,
  ignores corrupt records + `subagents` sidechains, never follows symlinks, and does
  filesystem work OFF the UI thread. When `[context] enabled = false`, nothing under
  `~/.claude/projects` is opened.
- Failed refresh keeps last snapshot visible + stale marker — never blank the panel.
- Refresh throttling: Claude/Codex fetches are internally 300s cache TTL even though
  the TUI ticks at 60s. Don't fetch on every tick.
