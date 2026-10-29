# AI Usage Bar for GNOME Shell

A native GNOME 45-50 top-panel indicator for
[`ai-usagebar-omarchy`](../README.md). One `ai-usagebar-omarchy usage --json`
fetch per poll drives everything: the bar tiles every working account next
to the clock/network, and a click dropdown projects each account's full
usage report.

The extension is deliberately a frontend, the same rule as the project's
other native surfaces. The Rust binary owns configuration, credentials,
provider names, money, and metrics; this extension only renders the report
JSON. Waybar users get the same data through `--vendor all`; the extension
exists because Waybar cannot dock into the GNOME top bar.

<!-- TODO: screenshot (no capture exists yet; add bar, dropdown, and
     Preferences images when a GNOME session is available; do not
     fabricate one) -->

## The bar

Every account that reads successfully gets one compact tile, separated by
`│` (text form shown; with logos on, each tile leads with the provider's
icon):

```
kimi 62%↺5.3h 18%↺2.3d │ anthropic 42%↺2h 15%↺2d

- Each tile leads with its provider logo when a bundled asset ships; a
  named account adds its own name beside the logo, and a vendor's lone
  default account is the logo alone. Without an asset the provider name
  stands in as text.
- The tile row sits on a translucent rounded backdrop plate — the same
  plate the Omarchy bar draws behind its tiles — so every figure stays
  legible and the tiles read as one group. The robot-icon fallback states
  carry no plate.
- Each figure is colored independently: the percentage by its own window's
  remaining band (green ≥50% remaining, yellow <50%, orange <20%, red <5%)
  and its `↺` countdown by how much of the window's TIME is left (1h of a
  5h window counts as 20%, 1.1d of 7d as 15.7%) on that same band scale —
  so a green 5h figure can sit beside an orange weekly one in the same
  tile, and each clock escalates on its own; the account tag stays neutral,
  and a critical part also goes bold so the alert survives color
  blindness.
- Alarms are per-account and never bleed across tiles: a tile's neutral
  text turns red only when its OWN account is broken, stale, or critical —
  one vendor maxing a window cannot redden another vendor's tag or balance
  figure. The single-account robot fallback alarms on a refresh failure, a
  vendor filter matching nothing, or the shown account's own urgency.
- Countdowns are compact one-unit forms (`42m`, `5.3h`, `2.3d`), each
  marked by `↺`. A window that has not started yet keeps its slot as
  `0%↺-` so it lines up with the figures that have timers.
- Balance-style accounts without quota windows (OpenRouter, DeepSeek, …)
  show their single headline figure instead.
- Exhausted accounts (every window maxed, or a balance at zero) leave the
  bar until their next report shows room again; the dropdown keeps them.
- Unconfigured or broken accounts never tile. When nothing can tile
  (tiling off, still loading, or every account exhausted or broken) the
  bar collapses to the robot icon alone.
- **Show remaining** (default on) flips every figure between what is left
  of the window and what is used.

## Controls

- Left-click opens the dropdown.
- Mouse wheel or middle-click cycles the account selection across the
  tileable accounts, with wraparound; the choice is persisted and restored
  across shell reloads.
- Right-click deliberately has no handler.

## The dropdown

The click dropdown tiles ONE TAB PER KEY (account) — one per working API
key or login, not one per vendor:

- A **tab row** at the top: every account that actually reads, sorted by
  vendor & key (a vendor's keys stay adjacent; the vendor default leads,
  numbered keys compare numerically). Each tab is labeled by the
  account's own report name (`Kimi`, `Z.AI · 1`, `Z.AI · 2`). A gear
  button at the row's right end opens Preferences (its Accounts page
  manages API keys) — the dropdown's settings entry. Selecting a tab
  shows that key's usage and persists the choice as the selection (the
  same one the wheel cycle writes).
- Inside a tab: a header with the account name and plan (marked as
  cached when the data is stale), then the report's ordered rows:
  metric rows with band-colored values and meter bars, text rows, block
  rows, and spacers. Window metrics reuse the bar's remaining bands;
  critical rows carry a warning glyph so the alert survives color
  blindness.
- Unconfigured vendors render nothing — no tab, no rows. Only a report
  where EVERY account is unconfigured shows the fresh-install row that
  opens Preferences. A configured-but-broken account (its key exists but
  the fetch errored) keeps one calm error row below the tabs, and a
  vendor filter that matches no account keeps its own remedy row.
- Live countdowns: a 30-second ticker rewrites every reset label and the
  footer in place, so an open menu stays accurate between network
  refreshes. Each countdown names the local clock time the window reopens
  (`Resets in 2h 0m · 14:00`, dated when it lands on another day).
- An updated-at footer from the newest fetch (`Updated 5m ago`).
- Menu items at the bottom: Refresh now and Preferences.

Opening the menu refreshes only when the data is older than the refresh
interval. A failed refresh keeps the previous report on screen and says
so.

## Requirements

- GNOME Shell 45-50.
- The `ai-usagebar-omarchy` binary, resolved in this order: the Binary
  path setting, `PATH`, then every `install-bin.sh` destination
  (`~/.local/bin`, `~/.cargo/bin`, mise shims — the legacy `ai-usagebar`
  name also resolves, for AUR installs). Install it per the
  [main README](../README.md) (`cargo install`, AUR, or release binaries);
  `./scripts/install-gnome.sh` installs it as its first step, and a
  **Refresh now** picks a freshly installed binary up without a shell
  restart.
- Optional: a Nerd Font as your monospace font. The only Nerd Font glyph
  in the UI is the critical marker `󰀪` on urgent metric rows; without
  the font it renders as tofu and everything else is unaffected.

## Install

```bash
./install.sh
# then reload the shell:
#   X11      → Alt+F2, type 'r', Enter
#   Wayland  → log out / in
gnome-extensions enable ai-usagebar@kylelee.github.io
```

`install.sh` compiles the GSettings schema and copies this folder into
`~/.local/share/gnome-shell/extensions/` — a real copy, so the installed
extension keeps working after the checkout moves (re-run the script to
redeploy code changes). A manual install MUST run
`glib-compile-schemas schemas/` itself before enabling: without the
compiled schema the extension cannot load its settings and will not
start.

Settings dialog:

```bash
gnome-extensions prefs ai-usagebar@kylelee.github.io
```

## Settings

All settings are extension-local GSettings (under
`/org/gnome/shell/extensions/ai-usagebar/`). None of them touch
`~/.config/ai-usagebar-omarchy/config.toml`; that file belongs to the
binary.

| Key | Default | Meaning |
|---|---|---|
| `refresh-interval` | 60 | Seconds between `usage --json` polls, clamped to 30-3600 |
| `vendor` | `''` | Entry filter: `''` tiles every configured account, a vendor slug (`anthropic`) shows that vendor's accounts, an exact entry id (`anthropic@work`) pins one account. A display filter only: the whole report is always fetched, so changing it re-renders without refetching |
| `binary-path` | `''` | Absolute path to the binary; empty = auto-detect on `PATH`, then `~/.cargo/bin` |
| `panel-box` | `right` | Panel area: `left`, `center`, or `right` (next to the network/clock) |
| `panel-index` | 0 | Position within the chosen area (0 = leftmost, max 20) |
| `show-remaining` | `true` | Tiles show what is left of each window instead of the used percentage |
| `bar-tiled` | `true` | Tile every working account side by side; off collapses the bar to the selected account alone |
| `show-logos` | `true` | Lead each tile with its provider logo |
| `color-low` | `#98c379` | Figures with at least 50% remaining |
| `color-mid` | `#e5c07b` | Figures with <50% remaining |
| `color-high` | `#d08770` | Figures with <20% remaining |
| `color-critical` | `#e06c75` | Figures with <5% remaining |
| `color-empty` | `#3e4451` | The meter track in the dropdown |
| `last-selected-entry` | `''` | Internal: entry id of the last selection, written by the extension and restored across reloads |
| `config-rev` | 0 | Internal: bumped by Preferences after every successful settings apply; the extension refreshes when it changes |

Panel position changes apply immediately. Color changes land on the next
render; an invalid hand-edited hex value degrades to neutral text rather
than breaking the markup.

## Preferences

The dialog's first page carries the display rows: the three toggles, the
five band colors, the refresh interval, the vendor filter, the binary
path, and the panel position. The vendor filter row repopulates itself
from a live report (providers first, then each named account), and a
stored value the report no longer contains is cleared for real.

### Accounts

The Accounts page is data-driven over the binary's settings bridge; the
extension never reads `config.toml` itself:

- `ai-usagebar-omarchy settings show` returns a non-secret JSON snapshot:
  the primary provider, its choices, and per-vendor account cards with
  their editable fields and key-presence booleans. It never prints a key.
- Every card carries its own **Apply**, which sends only that card's
  mutation to `ai-usagebar-omarchy settings apply`, the patch traveling
  over stdin. API keys exist only inside the prefs process and its pipe
  to the binary: never in argv, the environment, dconf, or the
  gnome-shell process. A blank key field keeps the current key; the
  explicit **Clear key** button removes an inline one. Saving a non-empty
  key also enables that vendor.
- Each vendor group has an **Add account** button; new accounts are
  auto-named by position (1, 2, …). Z.AI cards pick the billing type and,
  for team keys, ask for the two organization ids. Spend vendors
  (Anthropic API, OpenAI API) carry an optional monthly limit per card.
  Removing an account asks for confirmation.
- The primary provider selector at the top applies immediately as its own
  single-field patch.
- After every successful apply, Preferences bumps `config-rev` and the
  extension refreshes on the change, so saved accounts show up without a
  manual reload.
- Login flows that live outside the bridge: Claude and Codex log in
  through their own CLI in a terminal (the dialog offers to npm-install
  it to `~/.local`, no sudo, when it is missing). Antigravity is a local
  server with no separate login: keep the app, the IDE, or an interactive
  `agy` session running.
- A binary too old for the settings bridge gets a banner with **Retry**
  and an **Open terminal settings** fallback that runs the TUI in a
  terminal.

## How it renders

Each poll spawns `usage --json` asynchronously (`Gio.Subprocess` +
`communicate_utf8_async`, force-exited after 60 s) so the shell never
blocks, and hands stdout to `report-model.js`, a pure adapter with node
tests that normalizes and caps every field. `extension.js` is shell glue
only: tiles, menu, ticker, and the teardown of every timer and signal
handler in `disable()`. Older binaries that predate newer report fields
keep working: a missing logo falls back to the text tag, a missing
headline is recomputed locally.

## Development

```bash
node gnome-extension/report-model.test.mjs   # or: make desktop-test
```

The suite gates the pure adapter (report and settings parsing, tile and
selection behavior, patch building) with concrete fixtures and literal
expected strings, plus structural source gates that pin the wiring of
`extension.js`, `prefs.js`, and `subprocess-utils.js` from the outside
(node cannot import `gi://` modules). It runs on any OS with no npm
dependencies; CI runs it on Ubuntu and Windows.

Logs: `journalctl -f -o cat /usr/bin/gnome-shell`.
