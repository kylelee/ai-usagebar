//! The always-exits-0 orchestrator. Dispatches on `--vendor`, fetches a
//! snapshot, renders the right output mode, and prints. Catches every error
//! into a fallback `⚠` JSON / pretty line so Waybar never hides the module.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;

use crate::anthropic::{self, creds::CredsTarget, fetch::FetchOutcome};
use crate::anthropic_api;
use crate::antigravity;
use crate::cache::{Cache, DEFAULT_TTL};
use crate::config::Config;
use crate::cursor;
use crate::deepseek;
use crate::error::{AppError, Result};
use crate::grok;
use crate::kilo;
use crate::kimi;
use crate::kiro;
use crate::minimax;
use crate::moonshot;
use crate::novita;
use crate::openai;
use crate::openai_api;
use crate::openrouter;
use crate::pango::escape;
use crate::supergrok;
use crate::theme::Theme;
use crate::vendor::{HTTP_CLIENT_TIMEOUT, RenderOpts, VendorOutcome};
use crate::waybar::WaybarOutput;
use crate::widget::cli::{Cli, Vendor};
use crate::widget::pretty::print_pretty;
use crate::widget::render::{DEFAULT_FORMAT, RenderInput, render_anthropic};
use crate::zai;

/// Entry point — runs to completion and ALWAYS returns Ok with exit code 0
/// in the caller. Mirrors claudebar's `die()` invariant.
pub async fn run(cli: Cli) -> i32 {
    // Scroll-cycle short-circuit: don't render, just bump state + signal waybar.
    if cli.cycle_next || cli.cycle_prev {
        return run_cycle(&cli).await;
    }
    // First-run template notice, pretty terminals only: Waybar's JSON exec,
    // piped output, and `--watch` into a file must stay byte-stable.
    if !cli.output_json() && std::io::stdout().is_terminal() {
        let note = Config::load_with_note().ok().and_then(|(_, note)| note);
        note_template_seed(&mut std::io::stderr().lock(), note.as_deref());
    }
    if let Some(secs) = cli.watch {
        return run_watch(cli, secs).await;
    }
    run_once(&cli, &mut std::io::stdout()).await;
    0
}

/// Tell the user where the freshly seeded template landed. The path is
/// sanitized at the sink — config paths can carry account-derived components.
fn note_template_seed(err: &mut impl Write, note: Option<&std::path::Path>) {
    if let Some(path) = note {
        let _ = writeln!(
            err,
            "created config template at {}",
            crate::display::sanitize_untrusted_path(path)
        );
    }
}

/// Cycle to the next/prev enabled vendor and signal waybar to refresh.
/// Always exits 0 — Waybar swallows non-zero exits anyway.
async fn run_cycle(cli: &Cli) -> i32 {
    // Cycling against the *default* vendor set because the config failed to
    // parse would persist a selection the user never made. Do nothing instead;
    // the next render surfaces the config error through the `⚠` fallback.
    let Ok(config) = Config::load() else {
        return 0;
    };
    let enabled = config.enabled_vendors();
    if enabled.is_empty() {
        return 0;
    }
    // Starting point: current persisted vendor, else config.primary, else
    // anthropic. resolved_vendor() encodes that precedence, minus the
    // `cli.vendor` override (cycle commands ignore --vendor on purpose).
    let start = match config.ui.primary {
        Some(id) if enabled.contains(&id) => id,
        _ => enabled[0],
    };
    let delta = if cli.cycle_next { 1 } else { -1 };
    // Signalling after a *failed* persist told Waybar to re-render a selection
    // that was never written, so the bar redrew the same vendor and the scroll
    // looked like it had been swallowed. Only announce a change that happened.
    if crate::active::cycle(&enabled, start, delta).is_err() {
        return 0;
    }

    // Refresh the bar immediately. The Waybar module's `signal: 13` setting
    // means SIGRTMIN+13 re-runs the exec. SIGRTMIN is libc-dependent; the
    // shell-safe value on Linux glibc is signal 47 (= SIGRTMIN(34)+13).
    crate::waybar::request_refresh();
    0
}

/// `--watch` repaints in place, which only makes sense on a terminal. Piped or
/// redirected, the escape sequence is just garbage in the captured output.
/// Split out from the loop so the decision is testable without a real TTY.
fn should_clear_screen(stdout_is_tty: bool) -> bool {
    stdout_is_tty
}

/// The `--watch` hint belongs to the same terminal the screen clear repaints;
/// a piped `--watch` must not grow a line per interval. Split from the loop so
/// the gate is testable without a pty.
fn emit_watch_hint(err: &mut impl Write, gated: bool, secs: u64) {
    if gated {
        let _ = writeln!(err, "(re-rendering every {secs}s — press Ctrl-C to exit)");
    }
}

/// One stderr warning naming each `{[a-z_]+}` token that survived
/// substitution in the pretty bar text. `gated: false` is a no-op — JSON and
/// piped consumers get byte-stable output; split from the call site so the
/// gate is testable without a pty.
fn emit_pretty_warning(err: &mut impl Write, gated: bool, bar_text: &str) {
    if !gated {
        return;
    }
    let unknown = crate::format::unrecognized_placeholders(bar_text);
    if !unknown.is_empty() {
        let _ = writeln!(
            err,
            "warning: unrecognized placeholders: {}",
            unknown.join(", ")
        );
    }
}

async fn run_watch(cli: Cli, secs: u64) -> i32 {
    let interval = Duration::from_secs(secs.max(1));
    let clear = {
        use std::io::IsTerminal;
        should_clear_screen(std::io::stdout().is_terminal())
    };
    loop {
        if clear {
            // Clear screen + home cursor.
            print!("\x1b[2J\x1b[H");
        }
        let _ = std::io::stdout().flush();
        run_once(&cli, &mut std::io::stdout()).await;
        println!();
        emit_watch_hint(&mut std::io::stderr().lock(), clear, secs);
        tokio::select! {
            _ = tokio::time::sleep(interval) => continue,
            _ = tokio::signal::ctrl_c() => return 0,
        }
    }
}

async fn run_once(cli: &Cli, out: &mut impl Write) {
    let output = match build_output(cli).await {
        Ok(o) => o,
        Err(e) => {
            crate::diag::event(
                "widget/fallback",
                &format!("vendor={:?} {}", cli.vendor, e.user_message()),
            );
            fallback(&e, cli)
        }
    };

    if cli.output_json() {
        let _ = out.write_all(output.to_json_line().as_bytes());
    } else {
        let _ = print_pretty(out, &output);
        // A typo'd `{sesson_pct}` prints literally today; tell a human which
        // names failed to resolve. TTY-gated like the watch hint — a piped or
        // JSON run must not grow a stderr line.
        emit_pretty_warning(
            &mut std::io::stderr().lock(),
            std::io::stdout().is_terminal(),
            &output.text,
        );
    }
    let _ = out.flush();
}

async fn build_output(cli: &Cli) -> Result<WaybarOutput> {
    // A broken config is reported through the `⚠` fallback (still exit 0)
    // rather than silently reverting to the default vendor set — otherwise a
    // typo'd section shows another account's usage with no diagnostic.
    let config = Config::load()?;
    let vendor = cli.resolved_vendor(&config);
    // The aggregate tile view bypasses single-vendor validation entirely —
    // it has no vendor identity, no `--account`, and enumerates its own
    // eligibility (every enabled vendor/account).
    if matches!(vendor, Vendor::All) {
        return all_output(cli, &config).await;
    }
    validate_vendor_options(cli, vendor)?;
    if !dispatch_is_eligible(cli, &config, vendor) {
        return Err(disabled_vendor_error(vendor));
    }
    match vendor {
        // `all` never reaches the dispatch: build_output handled it above.
        // This arm exists only so the match stays exhaustive.
        Vendor::All => all_output(cli, &config).await,
        Vendor::Anthropic => anthropic_output(cli, &config).await,
        Vendor::AnthropicApi => anthropic_api_output(cli, &config).await,
        Vendor::OpenaiApi => openai_api_output(cli, &config).await,
        Vendor::Openrouter => openrouter_output(cli, &config).await,
        Vendor::Openai => openai_output(cli, &config).await,
        Vendor::Zai => zai_output(cli, &config).await,
        Vendor::Deepseek => deepseek_output(cli, &config).await,
        Vendor::Kimi => kimi_output(cli, &config).await,
        Vendor::Kilo => kilo_output(cli, &config).await,
        Vendor::Novita => novita_output(cli, &config).await,
        Vendor::Moonshot => moonshot_output(cli, &config).await,
        Vendor::Grok => grok_output(cli, &config).await,
        Vendor::Supergrok => supergrok_output(cli, &config).await,
        Vendor::Antigravity => antigravity_output(cli, &config).await,
        Vendor::Cursor => cursor_output(cli, &config).await,
        Vendor::Minimax => minimax_output(cli, &config).await,
        Vendor::Kiro => kiro_output(cli, &config).await,
        Vendor::OpenCodeGo => opencode_go_output(cli, &config).await,
    }
}

/// Pinned user-facing text; the section name must match config.rs's own hints.
fn disabled_vendor_error(vendor: Vendor) -> AppError {
    let section = vendor.to_id().map_or("all", |id| id.slug());
    AppError::Other(format!(
        "vendor {:?} is disabled in {}. Add `enabled = true` under [{section}]",
        vendor,
        crate::config::config_path_hint()
    ))
}

fn validate_vendor_options(cli: &Cli, vendor: Vendor) -> Result<()> {
    if cli.account.is_some()
        && !matches!(
            vendor,
            Vendor::Anthropic
                | Vendor::Openrouter
                | Vendor::Zai
                | Vendor::Kimi
                | Vendor::Deepseek
                | Vendor::Kilo
                | Vendor::Novita
                | Vendor::Moonshot
                | Vendor::Grok
                | Vendor::Minimax
                | Vendor::OpenCodeGo
                | Vendor::AnthropicApi
                | Vendor::OpenaiApi
        )
    {
        return Err(AppError::Other(
            "--account is supported only for the multi-account vendors".into(),
        ));
    }
    if cli.desktop && vendor != Vendor::Anthropic {
        return Err(AppError::Other(
            "--desktop is supported only for Claude accounts".into(),
        ));
    }
    Ok(())
}

/// Eligibility for a single-vendor dispatch: an explicit `--vendor` is an
/// intentional CLI opt-in, including for vendors that default to disabled
/// (such as Kimi). Unreachable for `all`, which never gets here.
fn dispatch_is_eligible(cli: &Cli, config: &Config, vendor: Vendor) -> bool {
    cli.has_explicit_vendor() || vendor.to_id().is_some_and(|id| config.is_enabled(id))
}

/// One vendor's fetch → render pipeline. A transient failure maps to the
/// calm `loading` output (Waybar keeps the module visible), any other error
/// propagates to the `⚠` fallback, and success renders through the vendor's
/// own `render` with the CLI's theme and options — the ten lines every
/// `*_output` below used to copy.
async fn fetch_and_render<O>(
    cli: &Cli,
    fetch: impl std::future::Future<Output = Result<O>>,
    render: fn(
        &VendorOutcome,
        &O::Snapshot,
        &Theme,
        &RenderOpts,
        chrono::DateTime<Utc>,
    ) -> WaybarOutput,
) -> Result<WaybarOutput>
where
    O: crate::vendor::VendorFetchOutcome,
{
    let outcome = match fetch.await {
        Ok(o) => o,
        Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
        Err(e) => return Err(e),
    };
    let (vendor_outcome, snap) = outcome.into_parts();
    Ok(render(
        &vendor_outcome,
        &snap,
        &theme_from_cli(cli),
        &RenderOpts::from(cli),
        Utc::now(),
    ))
}

async fn opencode_go_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::OpenCodeGo,
        "OpenCode Go",
        "opencode_go",
        &config.opencode_go.api_key_env,
        config.opencode_go.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "opencode-go", cli.account.as_deref())?;
    let endpoints = crate::opencode_go::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        crate::opencode_go::fetch::fetch_snapshot(
            &client,
            &api_key,
            &cache,
            &endpoints,
            DEFAULT_TTL,
        ),
        crate::opencode_go::vendor::render,
    )
    .await
}

/// Antigravity authenticates through whichever local product is running (the
/// 2.0 app, the `agy` CLI, or the IDE) — there is no API key to resolve.
async fn antigravity_output(cli: &Cli, _config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "antigravity")?;
    fetch_and_render(
        cli,
        antigravity::fetch_snapshot(&client, &cache, DEFAULT_TTL),
        antigravity::vendor::render,
    )
    .await
}

/// Cursor authenticates via a session token the Cursor IDE already wrote to
/// its local `state.vscdb` — no API key to resolve, but (unlike Antigravity)
/// a real on-disk path that can be overridden, mirroring OpenAI's
/// `codex_auth_path`.
async fn cursor_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "cursor")?;
    let db_path = match config.cursor.db_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => cursor::db::default_db_path()?,
    };
    let agent_auth_path = match config.cursor.agent_auth_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => cursor::db::default_agent_auth_path()?,
    };
    let endpoints = cursor::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        cursor::fetch_snapshot(
            &client,
            &db_path,
            &agent_auth_path,
            &cache,
            &endpoints,
            DEFAULT_TTL,
        ),
        cursor::vendor::render,
    )
    .await
}

/// Kiro CLI authenticates via the AWS SSO OIDC session kiro-cli already wrote
/// to its own local `data.sqlite3` — no API key to resolve, but (like Cursor)
/// a real on-disk path that can be overridden.
async fn kiro_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "kiro")?;
    let db_path = match config.kiro.db_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => kiro::db::default_db_path()?,
    };
    fetch_and_render(
        cli,
        kiro::fetch_snapshot(&client, &db_path, &cache, DEFAULT_TTL),
        kiro::vendor::render,
    )
    .await
}

async fn grok_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::Grok,
        "Grok",
        "grok",
        &config.grok.api_key_env,
        config.grok.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "grok", cli.account.as_deref())?;
    let endpoints = grok::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        grok::fetch_snapshot(
            &client,
            &api_key,
            &cache,
            &endpoints,
            DEFAULT_TTL,
            config.grok.team_id.as_deref(),
        ),
        grok::vendor::render,
    )
    .await
}

/// SuperGrok delegates auth and billing transport to the official Grok Build
/// ACP process — no token or API key is parsed, cached, refreshed, or placed
/// in an ACP message by ai-usagebar.
async fn supergrok_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let cache = vendor_cache(cli, "supergrok")?;
    let scope_paths = supergrok::scope::ScopePaths::with_overrides(
        config.supergrok.auth_path.as_deref(),
        config.supergrok.config_path.as_deref(),
    )?;
    fetch_and_render(
        cli,
        supergrok::fetch_snapshot(
            &config.supergrok.grok_binary,
            &scope_paths,
            &cache,
            DEFAULT_TTL,
        ),
        supergrok::vendor::render,
    )
    .await
}

async fn moonshot_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::Moonshot,
        "Moonshot",
        "moonshot",
        &config.moonshot.api_key_env,
        config.moonshot.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "moonshot", cli.account.as_deref())?;
    let (endpoints, currency) = moonshot::fetch::Endpoints::for_region(&config.moonshot.region);
    fetch_and_render(
        cli,
        moonshot::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL, currency),
        moonshot::vendor::render,
    )
    .await
}

async fn minimax_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::Minimax,
        "MiniMax",
        "minimax",
        &config.minimax.api_key_env,
        config.minimax.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "minimax", cli.account.as_deref())?;
    let endpoints = minimax::fetch::Endpoints::for_region(&config.minimax.region);
    fetch_and_render(
        cli,
        minimax::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL),
        minimax::vendor::render,
    )
    .await
}

async fn novita_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::Novita,
        "Novita",
        "novita",
        &config.novita.api_key_env,
        config.novita.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "novita", cli.account.as_deref())?;
    let endpoints = novita::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        novita::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL),
        novita::vendor::render,
    )
    .await
}

async fn kilo_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::Kilo,
        "Kilo",
        "kilo",
        &config.kilo.api_key_env,
        config.kilo.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "kilo", cli.account.as_deref())?;
    let endpoints = kilo::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        kilo::fetch_snapshot(
            &client,
            &api_key,
            &cache,
            &endpoints,
            DEFAULT_TTL,
            config.kilo.organization_id.as_deref(),
        ),
        kilo::vendor::render,
    )
    .await
}

async fn openai_api_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    // Numbered accounts carry their own Admin key and MAY carry their own
    // monthly limit; GENTLE_TTL — the costs data is daily-bucket reporting.
    let key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::OpenaiApi,
        "OpenAI API",
        "openai_api",
        &config.openai_api.api_key_env,
        config.openai_api.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "openai_api", cli.account.as_deref())?;
    let limit = match cli.account.as_deref().and_then(|label| {
        config
            .key_accounts(crate::vendor::VendorId::OpenaiApi)
            .and_then(|accounts| accounts.iter().find(|a| a.label == label))
    }) {
        Some(account) => account.monthly_limit.or(config.openai_api.monthly_limit),
        None => config.openai_api.monthly_limit,
    };
    let endpoints = openai_api::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        openai_api::fetch_snapshot(
            &client,
            &key,
            &cache,
            &endpoints,
            crate::cache::GENTLE_TTL,
            limit,
        ),
        openai_api::vendor::render,
    )
    .await
}

async fn anthropic_api_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    // Numbered accounts carry their own Admin key and MAY carry their own
    // monthly limit (each key is its own organization); the section limit
    // stays the default account's.
    let key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::AnthropicApi,
        "Anthropic_API",
        "anthropic_api",
        &config.anthropic_api.api_key_env,
        config.anthropic_api.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "anthropic_api", cli.account.as_deref())?;
    let limit = match cli.account.as_deref().and_then(|label| {
        config
            .key_accounts(crate::vendor::VendorId::AnthropicApi)
            .and_then(|accounts| accounts.iter().find(|a| a.label == label))
    }) {
        Some(account) => account.monthly_limit.or(config.anthropic_api.monthly_limit),
        None => config.anthropic_api.monthly_limit,
    };
    let endpoints = anthropic_api::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        anthropic_api::fetch_snapshot(&client, &key, &cache, &endpoints, DEFAULT_TTL, limit),
        anthropic_api::vendor::render,
    )
    .await
}

/// Resolve a Codex login and its cache as one identity, the way
/// [`openrouter_target`] does for a key. The default login keeps the historical
/// vendor-root cache; each named account is isolated below `openai/<label>`, so
/// two ChatGPT subscriptions never serve each other's usage from a warm cache.
fn openai_target(cli: &Cli, config: &Config) -> Result<(std::path::PathBuf, Cache)> {
    let label = cli.account.as_deref();
    let creds_path = config.openai.resolve_auth_path(label)?;
    let cache = match (cli.cache_dir.as_deref(), label) {
        (Some(root), Some(label)) => Cache::at(root.join("openai").join(label)),
        (Some(root), None) => Cache::at(root.join("openai")),
        (None, Some(label)) => Cache::for_vendor_account("openai", label)?,
        (None, None) => Cache::for_vendor("openai")?,
    };
    Ok((creds_path, cache))
}

async fn openai_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let (creds_path, cache) = openai_target(cli, config)?;
    let endpoints = openai::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        openai::fetch_snapshot(
            &client,
            &creds_path,
            &cache,
            &endpoints,
            crate::cache::GENTLE_TTL,
        ),
        openai::vendor::render,
    )
    .await
}

/// Global refresh budget for one `--vendor all` pass, from loop start.
/// Fetches are sequential and each can wait out its per-vendor HTTP timeout
/// (5s antigravity / 10s default / 15s admin-API), so a cold start with many
/// tabs could otherwise stall the bar for minutes.
const AGGREGATE_DEADLINE_SECS: u64 = 45;

/// `--vendor all`: every enabled vendor and account, tiled in one module.
/// Fetches sequentially through the shared per-vendor caches (the report's
/// rationale: parallel bursts hit provider rate limits for no gain), then
/// renders tiles + a combined popup from the same section projection the
/// TUI and `usage --json` use.
async fn all_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let tabs = crate::tui::app::tabs_with_desktop(config);
    if tabs.is_empty() {
        return Err(AppError::Other(format!(
            "no vendors enabled in {}",
            crate::config::config_path_hint()
        )));
    }
    let client = http_client()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(AGGREGATE_DEADLINE_SECS);
    let entries = aggregate_entries_with_deadline(&tabs, deadline, |tab| {
        let client = &client;
        async move { crate::tui::app::refresh_one(client, config, &tab).await }
    })
    .await;
    Ok(crate::widget::aggregate::render(
        &entries,
        &theme_from_cli(cli),
        &RenderOpts::from(cli),
        Utc::now(),
    ))
}

/// The `--vendor all` per-tab loop, extracted so the deadline logic is
/// unit-testable with injected fetches: the tabs are refreshed sequentially,
/// each refresh wrapped in `tokio::time::timeout(remaining_budget)` where
/// remaining_budget = deadline − now. Once the deadline has passed,
/// un-started tabs are never live-fetched — they render
/// [`deferred_tab_state`] instead of stalling the bar.
async fn aggregate_entries_with_deadline<F, Fut>(
    tabs: &[crate::tui::app::TabId],
    deadline: std::time::Instant,
    fetch: F,
) -> Vec<crate::widget::aggregate::AggregateEntry>
where
    F: Fn(crate::tui::app::TabId) -> Fut,
    Fut: std::future::Future<Output = crate::tui::app::TabState>,
{
    let now = Utc::now();
    let mut entries = Vec::with_capacity(tabs.len());
    for tab in tabs {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let state = if remaining.is_zero() {
            deferred_tab_state(tab)
        } else {
            match tokio::time::timeout(remaining, fetch(tab.clone())).await {
                Ok(state) => state,
                Err(_budget_spent) => deferred_tab_state(tab),
            }
        };
        entries.push(crate::widget::aggregate::AggregateEntry::from_tab(
            tab, &state, now,
        ));
    }
    entries
}

/// State for a tab the deadline cut off. There is no public cache-only
/// payload→snapshot seam (every vendor's `parse_cache` is private to its
/// module), so a deferred tab cannot rebuild its on-disk snapshot here
/// without duplicating tui::app's whole per-vendor dispatch; it states
/// plainly that it was not checked — the popup keeps the row visible and
/// the next refresh cycle re-fetches it normally.
fn deferred_tab_state(tab: &crate::tui::app::TabId) -> crate::tui::app::TabState {
    crate::tui::app::TabState::Error(format!(
        "⚠ {} not checked (cold start deadline)",
        tab.vendor.short_name()
    ))
}

async fn zai_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    // `--account <label>` selects a `[[zai.accounts]]` entry (own billing
    // type, site, team scope, and cache); without it the default `[zai]`
    // key resolves exactly as before — including a team type set on the
    // section itself.
    let plan = config.zai.resolve_account(cli.account.as_deref())?;
    let client = http_client()?;
    let cache = account_cache(cli, "zai", cli.account.as_deref())?;
    let endpoints = zai::fetch::Endpoints::for_site(plan.site);
    fetch_and_render(
        cli,
        zai::fetch::fetch_snapshot_plan(
            &client,
            &plan,
            &cache,
            &endpoints,
            DEFAULT_TTL,
            chrono::Utc::now(),
        ),
        zai::vendor::render,
    )
    .await
}

async fn openrouter_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let (api_key, cache) = openrouter_target(cli, config)?;
    let client = http_client()?;
    let endpoints = openrouter::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        openrouter::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL),
        openrouter::vendor::render,
    )
    .await
}

/// Resolve an OpenRouter key and its cache as one identity. The unnamed key
/// keeps the historical vendor-root cache; each named key is isolated below
/// `openrouter/<label>` so fresh data can never cross accounts.
fn openrouter_target(cli: &Cli, config: &Config) -> Result<(String, Cache)> {
    let label = cli.account.as_deref();
    let api_key = config.openrouter.resolve_api_key(label)?;
    Ok((api_key, account_cache(cli, "openrouter", label)?))
}

/// Cache resolution shared by every account-capable vendor: `--cache-dir`
/// overrides the root (its `<vendor>` subdir, plus `/<label>` for a named
/// account), otherwise the platform default with the same account split.
/// The default account keeps its historical `~/.cache/ai-usagebar-omarchy/<vendor>`
/// path so its cache never moves.
/// Resolve the API key for a key-account vendor's single-vendor run:
/// `--account N` picks the numbered account's key, otherwise the section
/// default — the shared rule every key vendor follows.
fn key_account_api_key(
    cli: &Cli,
    config: &Config,
    vendor: crate::vendor::VendorId,
    display: &str,
    section: &str,
    default_env: &str,
    default_key: Option<&str>,
) -> Result<String> {
    match cli.account.as_deref() {
        Some(label) => crate::config::resolve_key_account(
            display,
            section,
            config.key_accounts(vendor).unwrap_or_default(),
            label,
        ),
        None => crate::config::resolve_api_key(display, default_env, default_key),
    }
}

fn account_cache(cli: &Cli, vendor: &str, label: Option<&str>) -> Result<Cache> {
    Ok(match (cli.cache_dir.as_deref(), label) {
        (Some(root), Some(label)) => Cache::at(root.join(vendor).join(label)),
        (Some(root), None) => Cache::at(root.join(vendor)),
        (None, Some(label)) => Cache::for_vendor_account(vendor, label)?,
        (None, None) => Cache::for_vendor(vendor)?,
    })
}

async fn deepseek_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = key_account_api_key(
        cli,
        config,
        crate::vendor::VendorId::Deepseek,
        "DeepSeek",
        "deepseek",
        &config.deepseek.api_key_env,
        config.deepseek.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = account_cache(cli, "deepseek", cli.account.as_deref())?;
    let endpoints = deepseek::fetch::Endpoints::default();
    fetch_and_render(
        cli,
        deepseek::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL),
        deepseek::vendor::render,
    )
    .await
}

async fn kimi_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let (auth, endpoints) = kimi::resolve_auth_for(&config.kimi, cli.account.as_deref())?;
    let client = http_client()?;
    let cache = account_cache(cli, "kimi", cli.account.as_deref())?;
    fetch_and_render(
        cli,
        kimi::fetch::fetch_snapshot_with_auth(&client, &auth, &cache, &endpoints, DEFAULT_TTL),
        kimi::vendor::render,
    )
    .await
}

async fn anthropic_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let (creds_target, cache) = anthropic_target(cli, config)?;
    let endpoints = anthropic::fetch::Endpoints::default();
    let outcome = match anthropic::fetch_snapshot(
        &client,
        &creds_target,
        &cache,
        &endpoints,
        crate::cache::GENTLE_TTL,
    )
    .await
    {
        Ok(o) => o,
        Err(e) if e.is_transient() => {
            // Mirror claudebar's `loading_network` path.
            return Ok(WaybarOutput::loading(cli.icon.as_deref()));
        }
        Err(e) => return Err(e),
    };

    let theme = theme_from_cli(cli);

    Ok(render_with_theme(&outcome, &theme, cli))
}

fn render_with_theme(outcome: &FetchOutcome, theme: &Theme, cli: &Cli) -> WaybarOutput {
    let format_owned = cli
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let input = RenderInput {
        outcome,
        theme,
        format: &format_owned,
        tooltip_format: cli.tooltip_format.as_deref(),
        icon: cli.icon.as_deref(),
        pace_tolerance: cli.pace_tolerance,
        format_pace_color: cli.format_pace_color,
        tooltip_pace_pts: cli.tooltip_pace_pts,
        now: Utc::now(),
    };
    render_anthropic(&input)
}

pub(crate) fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_CLIENT_TIMEOUT)
        .redirect(crate::vendor::same_origin_redirect_policy())
        .build()
        .map_err(|e| AppError::Other(format!("http client init: {e}")))
}

fn vendor_cache(cli: &Cli, vendor: &str) -> Result<Cache> {
    match cli.cache_dir.as_deref() {
        Some(p) => Ok(Cache::at(p.join(vendor))),
        None => Cache::for_vendor(vendor),
    }
}

/// Resolve the Anthropic credentials target + cache for this run, honoring
/// `--account` (issue #14). `--cache-dir` still overrides the cache location;
/// `--account <label>` selects a configured extra account (its credentials
/// file and an `anthropic/<label>` cache subdir); with neither, the default
/// account behaves byte-identically to before.
fn anthropic_target(cli: &Cli, config: &Config) -> Result<(CredsTarget, Cache)> {
    match cli.account.as_deref() {
        // `--account <label> --desktop`: read the Claude Desktop app's own token
        // for that saved profile (the menu bar's overview path). `--cache-dir`
        // still relocates the cache for scripted/multi-monitor setups.
        Some(label) if cli.desktop => {
            let (target, default_cache) =
                crate::anthropic::desktop_creds::account_target(config, label)?;
            let cache = match cli.cache_dir.as_deref() {
                Some(p) => Cache::at(p.join("anthropic").join(label)),
                None => default_cache,
            };
            Ok((target, cache))
        }
        Some(label) => named_account_target(cli, config, label),
        None => Ok((
            anthropic_default_creds(cli, config)?,
            vendor_cache(cli, "anthropic")?,
        )),
    }
}

/// A configured extra account (`--account <label>`): its own credentials file
/// and an isolated `anthropic/<label>` cache. `--account` conflicts with
/// `--creds-path`, so the account's file is the only credentials source here —
/// an Explicit target, so a missing/broken file fails loudly instead of
/// falling back to the (different account's) macOS Keychain item (#15).
fn named_account_target(cli: &Cli, config: &Config, label: &str) -> Result<(CredsTarget, Cache)> {
    let (creds, default_cache) = config.anthropic.account_target(label)?;
    // `--cache-dir` still wins for scripted/multi-monitor setups; otherwise use
    // the account's default `anthropic/<label>` cache from account_target.
    let cache = match cli.cache_dir.as_deref() {
        Some(p) => Cache::at(p.join("anthropic").join(label)),
        None => default_cache,
    };
    Ok((creds, cache))
}

/// Default-account credentials: `--creds-path` wins, then the singular
/// `[anthropic] credentials_path`, then the platform default file. This is the
/// pre-#14 resolution, kept intact so default output never changes. Only the
/// platform-default file is a `Default` target (eligible for the macOS
/// Keychain fallback); the two overrides are explicit user choices, read
/// strictly.
fn anthropic_default_creds(cli: &Cli, config: &Config) -> Result<CredsTarget> {
    if let Some(p) = cli.creds_path.as_deref() {
        return Ok(CredsTarget::Explicit(p.to_path_buf()));
    }
    if let Some(p) = config.anthropic.credentials_path.as_deref() {
        return Ok(CredsTarget::Explicit(p.to_path_buf()));
    }
    Ok(CredsTarget::Default(anthropic::creds::default_path()?))
}

fn theme_from_cli(cli: &Cli) -> Theme {
    Theme::default().merged_with_omarchy().with_overrides(
        cli.color_low.clone(),
        cli.color_mid.clone(),
        cli.color_high.clone(),
        cli.color_critical.clone(),
    )
}

/// Fallback output when everything goes wrong — always renders a `⚠` widget.
fn fallback(err: &AppError, _cli: &Cli) -> WaybarOutput {
    let tooltip = match err {
        AppError::Credentials(m) | AppError::Unconfigured(m) => format!("Credentials error.\n{m}"),
        AppError::Http { status, .. } if matches!(status, 401 | 403) => {
            format!("HTTP {status}\n{}", crate::error::AUTH_FAILURE_MESSAGE)
        }
        AppError::Http { status, body } => format!("HTTP {status}\n{body}"),
        AppError::Schema(m) => format!("API schema drift.\n{m}"),
        AppError::Io { path, source } => format!("I/O error at {}.\n{source}", path.display()),
        AppError::Other(m) | AppError::Transport(m) => m.clone(),
        AppError::Json(e) => format!("JSON error: {e}"),
        AppError::Toml(e) => format!("TOML error: {e}"),
        AppError::IoBare(e) => format!("I/O error: {e}"),
    };
    // Tooltips are Pango markup. Escape error text before serializing it so an
    // error cannot inject markup; serde still produces valid one-line JSON.
    WaybarOutput::error(&escape(&tooltip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::anthropic::fetch::FetchOutcome;
    use crate::usage::{AnthropicSnapshot, UsageWindow};

    fn cli_default() -> Cli {
        // clap's Default isn't derived for us; build from parse_from with no
        // args to get the canonical defaults.
        use clap::Parser;
        Cli::parse_from(["ai-usagebar-omarchy"])
    }

    fn dummy_outcome() -> FetchOutcome {
        FetchOutcome {
            snapshot: AnthropicSnapshot {
                plan: "Test".into(),
                session: UsageWindow {
                    utilization_pct: 25,
                    resets_at: None,
                    window_duration: chrono::Duration::hours(5),
                },
                weekly: UsageWindow {
                    utilization_pct: 10,
                    resets_at: None,
                    window_duration: chrono::Duration::days(7),
                },
                sonnet: None,
                scoped: vec![],
                extra: None,
            },
            stale: false,
            last_error: None,
            cache_age: None,
        }
    }

    #[test]
    fn render_with_theme_uses_cli_overrides() {
        let cli = {
            let mut c = cli_default();
            c.format = Some("test:{session_pct}".into());
            c.color_low = Some("#123456".into());
            c
        };
        let outcome = dummy_outcome();
        let theme = Theme::default().with_overrides(cli.color_low.clone(), None, None, None);
        let out = render_with_theme(&outcome, &theme, &cli);
        // Bar text should contain our format substitution, wrapped in the
        // overridden low-color span.
        assert!(out.text.contains("test:25"));
        assert!(out.text.contains("#123456"));
    }

    #[test]
    fn fallback_wraps_credentials_error_in_warning() {
        let err = AppError::Credentials("missing token".into());
        let out = fallback(&err, &cli_default());
        assert_eq!(out.text, "⚠");
        assert!(out.tooltip.contains("missing token"));
    }

    #[test]
    fn watch_only_clears_the_screen_on_a_terminal() {
        // Redirected to a file or piped, the clear-screen escape is garbage in
        // the captured output.
        assert!(should_clear_screen(true));
        assert!(!should_clear_screen(false));
    }

    #[test]
    fn watch_hint_is_silent_when_the_tty_gate_is_closed() {
        let mut buf: Vec<u8> = Vec::new();
        emit_watch_hint(&mut buf, false, 5);
        assert!(buf.is_empty(), "piped --watch must not grow stderr");
    }

    #[test]
    fn watch_hint_names_ctrl_c_when_gated() {
        let mut buf: Vec<u8> = Vec::new();
        emit_watch_hint(&mut buf, true, 5);
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Ctrl-C"), "{out}");
        assert!(out.contains("every 5s"), "{out}");
    }

    #[test]
    fn placeholder_warning_is_silent_when_the_tty_gate_is_closed() {
        let mut buf: Vec<u8> = Vec::new();
        emit_pretty_warning(&mut buf, false, "{sesson_pct} 42%");
        assert!(buf.is_empty(), "non-TTY runs must not warn");
    }

    #[test]
    fn placeholder_warning_names_only_the_unknown_tokens_when_gated() {
        // Post-substitution text: the known key already became "42%" in the
        // bar text, so the warning names only the unknown token — deduplicated,
        // one line.
        let mut buf: Vec<u8> = Vec::new();
        emit_pretty_warning(&mut buf, true, "{sesson_pct} 42% {sesson_pct}");
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "warning: unrecognized placeholders: {sesson_pct}\n");
    }

    #[test]
    fn placeholder_warning_is_silent_for_clean_text() {
        let mut buf: Vec<u8> = Vec::new();
        emit_pretty_warning(&mut buf, true, "42% · 2h 05m");
        assert!(buf.is_empty());
    }

    #[test]
    fn template_seed_note_is_quiet_without_a_note_and_names_the_path_with_one() {
        let mut buf: Vec<u8> = Vec::new();
        note_template_seed(&mut buf, None);
        assert!(buf.is_empty());

        let mut buf: Vec<u8> = Vec::new();
        note_template_seed(&mut buf, Some(std::path::Path::new("/tmp/cfg.toml")));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.starts_with("created config template at "), "{out}");
        assert!(out.contains("/tmp/cfg.toml"), "{out}");
    }

    #[test]
    fn a_failed_cycle_persist_is_not_announced_to_waybar() {
        // Signalling after a failed persist made Waybar re-render the *same*
        // vendor, so the scroll looked swallowed. An empty vendor set is the
        // reachable failure: `cycle_at` errors and nothing is written.
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("active_vendor");
        let r = crate::active::cycle_at(&path, &[], crate::vendor::VendorId::Anthropic, 1);
        assert!(r.is_err());
        assert!(
            crate::active::read_from(&path).is_none(),
            "a failed cycle must not have persisted anything"
        );
    }

    #[test]
    fn fallback_reports_a_broken_config_without_breaking_exit_0() {
        // Propagating the config error must still land in the `⚠` fallback —
        // Waybar hides modules that exit non-zero, so a broken config has to
        // be *visible*, not fatal.
        let toml_err = toml::from_str::<Config>("[zai\nenabled = true\n").unwrap_err();
        let out = fallback(&AppError::Toml(toml_err), &cli_default());
        assert_eq!(out.text, "⚠");
        assert!(out.tooltip.contains("TOML error"));
    }

    #[test]
    fn fallback_escapes_pango_and_keeps_valid_json() {
        let out = fallback(
            &AppError::Other("bad <markup> & value".into()),
            &cli_default(),
        );
        assert_eq!(out.tooltip, "bad &lt;markup&gt; &amp; value");
        let json: serde_json::Value = serde_json::from_str(out.to_json_line().trim()).unwrap();
        assert_eq!(json["text"], "⚠");
        assert_eq!(json["tooltip"], "bad &lt;markup&gt; &amp; value");
    }

    #[test]
    fn fallback_does_not_expose_authentication_response_bodies() {
        let out = fallback(
            &AppError::Http {
                status: 401,
                body: "PANCEA user@example.test <credential>&token".into(),
            },
            &cli_default(),
        );
        assert!(out.tooltip.contains(crate::error::AUTH_FAILURE_MESSAGE));
        assert!(!out.tooltip.contains("PANCEA"));
        assert!(!out.tooltip.contains("user@example.test"));
        assert!(!out.tooltip.contains("&amp;token"));
    }

    #[test]
    fn explicit_kimi_is_eligible_when_disabled_in_config() {
        use clap::Parser;
        let cli = Cli::parse_from(["ai-usagebar", "--vendor", "kimi"]);
        let config = Config::default();
        let vendor = cli.resolve_vendor_with(&config, None);
        assert_eq!(vendor, Vendor::Kimi);
        assert!(dispatch_is_eligible(&cli, &config, vendor));
    }

    #[test]
    fn implicit_disabled_vendor_is_not_dispatch_eligible() {
        let cli = cli_default();
        let config = Config::default();
        assert!(!dispatch_is_eligible(&cli, &config, Vendor::Kimi));
        // Normal implicit resolution avoids that disabled vendor entirely.
        assert_eq!(cli.resolve_vendor_with(&config, None), Vendor::Anthropic);
    }

    #[test]
    fn disabled_vendor_error_suggests_enabling_the_section() {
        let msg = disabled_vendor_error(Vendor::Kimi).to_string();
        assert!(msg.contains("enabled = true"), "{msg}");
        assert!(msg.contains("[kimi]"), "{msg}");
    }

    #[test]
    fn disabled_vendor_error_uses_the_exact_opencode_go_section_name() {
        let msg = disabled_vendor_error(Vendor::OpenCodeGo).to_string();
        assert!(msg.contains("[opencode-go]"), "{msg}");
        assert!(!msg.contains("[opencode go]"), "{msg}");
    }

    // --- issue #14: multi-account Anthropic target resolution ---------------
    // All hermetic: paths are only *resolved*, never opened, and every cache
    // uses an explicit --cache-dir (Cache::at) so no test touches real XDG.

    fn cli_with(account: Option<&str>, creds: Option<&str>, cache: Option<&str>) -> Cli {
        let mut c = cli_default();
        c.account = account.map(str::to_string);
        c.creds_path = creds.map(PathBuf::from);
        c.cache_dir = cache.map(PathBuf::from);
        c
    }

    fn config_with_account(label: &str, creds: &str) -> Config {
        let mut config = Config::default();
        config
            .anthropic
            .accounts
            .push(crate::config::AnthropicAccount {
                label: label.into(),
                credentials_path: creds.into(),
            });
        config
    }

    #[test]
    fn default_account_target_is_unchanged() {
        // No --account: creds come from --creds-path and the cache stays at the
        // vendor root (…/anthropic) — byte-identical to the pre-#14 path. An
        // explicit --creds-path is a strict (non-Keychain) target.
        let cli = cli_with(None, Some("/tmp/creds.json"), Some("/tmp/cache"));
        let (creds, cache) = anthropic_target(&cli, &Config::default()).unwrap();
        assert_eq!(
            creds,
            CredsTarget::Explicit(PathBuf::from("/tmp/creds.json"))
        );
        assert_eq!(cache.dir(), std::path::Path::new("/tmp/cache/anthropic"));
    }

    #[test]
    fn default_account_without_overrides_is_keychain_eligible() {
        // No --account, no --creds-path, no config path → the platform-default
        // location, the only target eligible for the macOS Keychain fallback.
        let cli = cli_with(None, None, Some("/tmp/cache"));
        let (creds, _cache) = anthropic_target(&cli, &Config::default()).unwrap();
        assert!(matches!(creds, CredsTarget::Default(_)));
    }

    #[test]
    fn named_account_uses_its_creds_and_label_subdir() {
        // A named account's file is read strictly first; the Keychain fallback
        // (if the file is missing/unusable) is scoped to its own directory, so
        // it can never silently read a *different* account's item (#15).
        let config = config_with_account("work", "/creds/work.json");
        let cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        let (creds, cache) = anthropic_target(&cli, &config).unwrap();
        assert_eq!(
            creds,
            CredsTarget::Named {
                path: PathBuf::from("/creds/work.json"),
                config_dir: PathBuf::from("/creds"),
            }
        );
        assert_eq!(
            cache.dir(),
            std::path::Path::new("/tmp/cache/anthropic/work")
        );
    }

    #[test]
    fn unknown_account_errors_listing_known_labels() {
        let config = config_with_account("work", "/creds/work.json");
        let cli = cli_with(Some("nope"), None, Some("/tmp/cache"));
        let err = anthropic_target(&cli, &config).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("nope"), "names the bad label: {msg}");
        assert!(msg.contains("work"), "lists known labels: {msg}");
    }

    #[test]
    fn account_conflicts_with_creds_path() {
        use clap::Parser;
        let res = Cli::try_parse_from(["ai-usagebar", "--account", "work", "--creds-path", "/x"]);
        assert!(res.is_err(), "--account and --creds-path must conflict");
    }

    #[test]
    fn openrouter_named_account_uses_its_key_and_cache_subdir() {
        let mut config = Config::default();
        config
            .openrouter
            .accounts
            .push(crate::config::OpenRouterAccount {
                label: "work".into(),
                api_key_env: None,
                api_key: Some("work-key".into()),

                monthly_limit: None,
            });
        config
            .openrouter
            .accounts
            .push(crate::config::OpenRouterAccount {
                label: "personal".into(),
                api_key_env: None,
                api_key: Some("personal-key".into()),

                monthly_limit: None,
            });
        let root = tempfile::tempdir().unwrap();
        let root_str = root.path().to_str().unwrap();
        let cli = cli_with(Some("work"), None, Some(root_str));
        let (key, cache) = openrouter_target(&cli, &config).unwrap();
        assert_eq!(key, "work-key");
        assert_eq!(cache.dir(), root.path().join("openrouter/work"));
        cache.write_payload(b"work-only").unwrap();

        let personal_cli = cli_with(Some("personal"), None, Some(root_str));
        let (personal_key, personal_cache) = openrouter_target(&personal_cli, &config).unwrap();
        assert_eq!(personal_key, "personal-key");
        assert!(
            personal_cache.maybe_payload().unwrap().is_none(),
            "a named account must not reuse another account's fresh cache"
        );
    }

    #[test]
    fn openrouter_default_target_keeps_the_original_cache_path() {
        let mut config = Config::default();
        config.openrouter.api_key_env.clear();
        config.openrouter.api_key = Some("default-key".into());
        let cli = cli_with(None, None, Some("/tmp/cache"));
        let (key, cache) = openrouter_target(&cli, &config).unwrap();
        assert_eq!(key, "default-key");
        assert_eq!(cache.dir(), std::path::Path::new("/tmp/cache/openrouter"));
    }

    #[test]
    fn account_flag_rejects_unrelated_vendors() {
        let cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        // Local-session vendors have no accounts to address.
        assert!(validate_vendor_options(&cli, Vendor::Cursor).is_err());
        assert!(validate_vendor_options(&cli, Vendor::Antigravity).is_err());
        assert!(validate_vendor_options(&cli, Vendor::Supergrok).is_err());
        assert!(validate_vendor_options(&cli, Vendor::Anthropic).is_ok());
        assert!(validate_vendor_options(&cli, Vendor::Openrouter).is_ok());
        // Z.AI and Kimi grew `[[…accounts]]` arrays; they accept the flag too.
        assert!(validate_vendor_options(&cli, Vendor::Zai).is_ok());
        assert!(validate_vendor_options(&cli, Vendor::Kimi).is_ok());
        // Every key-based vendor is multi-account now.
        for vendor in [
            Vendor::Deepseek,
            Vendor::Kilo,
            Vendor::Novita,
            Vendor::Moonshot,
            Vendor::Grok,
            Vendor::Minimax,
            Vendor::OpenCodeGo,
            Vendor::AnthropicApi,
        ] {
            assert!(
                validate_vendor_options(&cli, vendor).is_ok(),
                "{vendor:?} should accept --account"
            );
        }
    }

    #[test]
    fn desktop_flag_remains_claude_only() {
        let mut cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        cli.desktop = true;
        assert!(validate_vendor_options(&cli, Vendor::Openrouter).is_err());
        assert!(validate_vendor_options(&cli, Vendor::Anthropic).is_ok());
    }

    // --- --vendor all: bounded cold start ---------------------------------
    // The deadline path is pinned here hermetically (injected deadline +
    // injected fetches); a live `--vendor all` run proves nothing.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::tui::app::{ReadyTab, TabId, TabState};
    use crate::usage::ZaiSnapshot;
    use crate::vendor::VendorId;

    fn zai_ready_state(pct: i32) -> TabState {
        TabState::Ready(Box::new(ReadyTab {
            snapshot: crate::usage::VendorSnapshot::Zai(ZaiSnapshot {
                plan: "GLM Coding Pro".into(),
                session: Some(UsageWindow {
                    utilization_pct: pct,
                    resets_at: None,
                    window_duration: chrono::Duration::hours(5),
                }),
                weekly: None,
                mcp: None,
                usage_stats: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }))
    }

    fn not_checked_row(short_name: &str) -> String {
        format!("⚠ {short_name} not checked (cold start deadline)")
    }

    /// Given a 50ms budget, one immediate tab, one never-resolving tab, and
    /// one tab that must never be started: the helper still returns within
    /// the budget, the fast tab is collected, and the cut-off tabs render
    /// the explicit not-checked row instead of stalling the bar.
    #[tokio::test]
    async fn aggregate_deadline_bounds_cold_start_and_skips_unstarted_tabs() {
        let tabs = vec![
            TabId::vendor(VendorId::Zai),
            TabId::vendor(VendorId::Kimi),
            TabId::vendor(VendorId::Kilo),
        ];
        let kilo_fetched = Arc::new(AtomicBool::new(false));
        let kilo_fetched_flag = kilo_fetched.clone();
        let fetch =
            move |tab: TabId| -> std::pin::Pin<Box<dyn std::future::Future<Output = TabState>>> {
                match tab.vendor {
                    VendorId::Zai => Box::pin(async { zai_ready_state(42) }),
                    // The hung-command probe: a vendor that never answers.
                    VendorId::Kimi => Box::pin(std::future::pending()),
                    _ => {
                        kilo_fetched_flag.store(true, Ordering::SeqCst);
                        Box::pin(async { TabState::Error("must not be live-fetched".into()) })
                    }
                }
            };

        let deadline = std::time::Instant::now() + Duration::from_millis(50);
        let started = std::time::Instant::now();
        let entries = aggregate_entries_with_deadline(&tabs, deadline, fetch).await;

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "helper overran the 50ms budget: {:?}",
            started.elapsed()
        );
        assert!(
            entries[0].error.is_none(),
            "fast tab not collected: {:?}",
            entries[0].error
        );
        assert_eq!(
            entries[1].error.as_deref(),
            Some(not_checked_row("kmi").as_str())
        );
        assert_eq!(
            entries[2].error.as_deref(),
            Some(not_checked_row("klo").as_str())
        );
        assert!(
            !kilo_fetched.load(Ordering::SeqCst),
            "a tab past the deadline was live-fetched"
        );
    }

    /// An already-spent deadline defers from the very first tab: no fetch
    /// call is made at all.
    #[tokio::test]
    async fn aggregate_deadline_defers_the_first_tab_without_fetching_it() {
        let tabs = vec![TabId::vendor(VendorId::Zai)];
        let zai_fetched = Arc::new(AtomicBool::new(false));
        let zai_fetched_flag = zai_fetched.clone();
        let fetch =
            move |tab: TabId| -> std::pin::Pin<Box<dyn std::future::Future<Output = TabState>>> {
                zai_fetched_flag.store(true, Ordering::SeqCst);
                Box::pin(async move {
                    let _ = tab;
                    TabState::Error("must not be live-fetched".into())
                })
            };

        let entries =
            aggregate_entries_with_deadline(&tabs, std::time::Instant::now(), fetch).await;

        assert!(!zai_fetched.load(Ordering::SeqCst));
        assert_eq!(
            entries[0].error.as_deref(),
            Some(not_checked_row("zai").as_str())
        );
    }

    /// Malformed-input pin: an empty tab list yields no entries, no panic.
    #[tokio::test]
    async fn aggregate_deadline_with_no_tabs_returns_no_entries() {
        let entries = aggregate_entries_with_deadline(
            &[],
            std::time::Instant::now(),
            |tab: TabId| async move {
                let _ = tab;
                TabState::Loading
            },
        )
        .await;
        assert!(entries.is_empty());
    }
}
