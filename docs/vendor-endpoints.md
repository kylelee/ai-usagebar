# Vendor endpoints and live tests

Some providers do not publish a stable usage API. ai-usagebar-omarchy keeps its parsers
defensive and includes opt-in live tests for catching response changes.

## Support matrix

| Vendor | Endpoint | What you see | Native desktop selector |
|---|---|---|---|
| **Claude** | `api.anthropic.com/api/oauth/usage` (undocumented) | Session (5h), Weekly (7d), model-scoped weekly (e.g. Fable), Extra usage $ | All frontends |
| **Codex** | `chatgpt.com/backend-api/wham/usage` (undocumented; used by official `codex` CLI) | Codex 5h and/or weekly, Code-review weekly, Credits | All frontends |
| **Z.AI** | `api.z.ai/api/monitor/usage/quota/limit` (undocumented) | Session 5h, Weekly 7d, MCP tools monthly | All frontends |
| **OpenRouter** | `openrouter.ai/api/v1/{credits,key}` (documented) | Balance, today/week/month spend, free vs paid tier | All frontends |
| **DeepSeek** | `api.deepseek.com/user/balance` (documented) | Balance, granted, topped-up credits | All frontends |
| **Kimi** | `api.kimi.com\|.ai/coding/v1/usages` (undocumented; community-confirmed), plus `auth.kimi.com\|.ai/api/oauth/token` to refresh a Kimi Code CLI login | Weekly subscription quota + 5h rolling rate-limit window | Omarchy/KDE/macOS |
| **MiniMax** | `api.minimax.io/v1/token_plan/remains` (official Token Plan quota route) | Token Plan rolling interval window + weekly, per model bucket (text, video) | Omarchy/KDE |
| **Kilo** | `api.kilo.ai/api/profile/balance` (undocumented; extension-internal) | Remaining credit balance ($) | Omarchy/KDE/macOS |
| **Novita** | `api.novita.ai/openapi/v1/billing/balance/detail` (documented) | Remaining credit balance ($) | Omarchy/KDE/macOS |
| **Moonshot** | `api.moonshot.ai\|.cn/v1/users/me/balance` (documented) | Account balance ($ on `.ai`, ¥ on `.cn`) | Omarchy/KDE/macOS |
| **Grok (xAI)** | `management-api.x.ai/v1/billing/teams/{team}/prepaid/balance` (Management API; documented) | Prepaid credit balance ($) | Omarchy/KDE/macOS |
| **SuperGrok** | Official Grok Build `x.ai/billing` ACP extension | Current weekly/monthly included-credit %, prepaid API balance, reset | Omarchy/KDE |
| **Anthropic API** | `api.anthropic.com/v1/organizations/cost_report` (Admin API; documented) | Month-to-date spend ($, excludes Priority Tier), optional spend-vs-limit % | Omarchy/KDE/macOS |
| **OpenAI API** | `api.openai.com/v1/organization/costs` (Admin API; documented) | Trailing-30-day billed $ grouped by line item (daily buckets), optional spend-vs-limit % | Omarchy/KDE |
| **Antigravity** | Local loopback RPC on a dynamically assigned port, discovered from whichever Antigravity product is running (2.0, the IDE, or an interactive `agy` session) via `/proc` (Linux), `lsof` (macOS), or the process/TCP table (Windows) | Two independent quota pools — Gemini, and Claude & GPT OSS — each with a 5-hour and a weekly window | All frontends |
| **Cursor** | `cursor.com/api/usage-summary` (undocumented; the dashboard's own frontend) | Two included-usage pools this billing cycle — Cursor Models (Auto/Composer) % and Other Models (named/API) % — plus plan, reset, on-demand | Omarchy/KDE/macOS |
| **Kiro CLI** | `codewhisperer.<region>.amazonaws.com` `GetUsageLimits` (undocumented; the same call kiro-cli's own `/usage` slash command makes) | Single credit pool this cycle — used/limit/%, plan, reset | Omarchy/KDE |
| **OpenCode Go** | `opencode.ai/zen/go/v1/usage` | Rolling, weekly, and monthly `percent` windows with absolute reset timestamps | Omarchy/KDE |

The Omarchy panel, the KDE plasmoid, and the GNOME Shell extension render
every enabled entry of the `usage --json` report, and the Waybar widget
(`--vendor all`) and the TUI cover every vendor, so "Omarchy/KDE" is the
floor for all 18. The macOS menubar selector covers 13 of the 18; OpenAI
API, SuperGrok, MiniMax, Kiro CLI, and OpenCode Go are missing there only.
The GNOME extension's earlier fixed-vendor interface was superseded by the
report-driven panel in v3.


## Stability notes

| Provider | Status |
|---|---|
| Claude | Undocumented usage endpoint, but used by the official `claude` CLI. Less fragile than a scraped web page. |
| Codex | Undocumented ChatGPT usage endpoint used by the official `codex` CLI. Windows are identified by duration instead of response position. |
| Z.AI | Reverse-engineered from a third-party plugin. Treat this as the most fragile integration. |
| Kimi | Community-confirmed `/coding/v1/usages` route used by third-party quota tools. Drift is possible. The refresh grant is the Kimi Code CLI's own documented-by-behaviour device-flow token endpoint, using the CLI's public client id. |
| Cursor | Undocumented endpoint called by Cursor's dashboard. Its shape may change with Cursor pricing. |
| MiniMax | The Token Plan route is official, but no formal response schema is published. |
| Kiro CLI | `GetUsageLimits` is the same undocumented CodeWhisperer operation used by kiro-cli's `/usage` command. AWS SSO OIDC `CreateToken`, used for refresh, is documented. |
| Antigravity | Best-effort local integration: with no Antigravity product running it shows the last cached figures, then an error once those age out. `ANTIGRAVITY_LS_ADDRESS` is a *first* candidate, not an exclusive one — discovered ports are still probed behind it. |

Codex's known five-hour and seven-day windows are matched by their reported
duration, not by `primary_window` or `secondary_window` position. This handles
both the normal response and the temporary
[weekly-only response](https://github.com/openai/codex/issues/32707) without a
config switch.

## Run the live tests

```bash
make smoke
```

All 18 vendors are covered by 21 `#[ignore]` live tests (Z.AI adds team and
pay-as-you-go variants; Kimi adds a subscription-login test). Every test skips
gracefully when its vendor is unconfigured — an unset API-key environment
variable, missing local credentials, or, for Antigravity, no local server
discovered — so with no credentials at all `make smoke` prints a skip line per
test and exits 0. Each vendor runs against its real endpoint as soon as its key
or login is present (the Kimi smoke test covers the API-key path; a
subscription login is exercised by `ai-usagebar-omarchy --vendor kimi`).

To test only Kimi:

```bash
cargo test --test live kimi_live -- --ignored --nocapture
```

The tests validate the fields used by ai-usagebar-omarchy and report which part of a
response changed.
