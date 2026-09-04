//! Neutral tab model shared by every frontend — hoisted from `tui/app.rs`
//! and `tui/panels.rs` so the `usage` report, the aggregate widget, and the
//! macOS menu bar do not depend on the terminal UI frontend. Owns: the tab
//! identity and enumeration (`TabId`, `tabs_from_config`, `tabs_with_desktop`),
//! the per-tab refresh pipeline (`refresh_one` and its per-vendor dispatch),
//! and the vendor-agnostic `Section` projection
//! (`sections_with_metadata_for`). UI-only concerns (drawing, focus,
//! keybindings, terminal rendering) stay in `tui`; the historical
//! `tui::app` / `tui::panels` paths re-export from here, so every
//! pre-existing import keeps resolving.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Client;

use crate::cache::DEFAULT_TTL;
use crate::config::Config;
use crate::countdown;
use crate::error::Result;
use crate::format::{local_time_hms, money, usd};
use crate::pacing::{self, PaceSeverity};
use crate::pango::severity_for;
use crate::usage::VendorSnapshot;
use crate::vendor::{VendorId, VendorOutcome};

/// What we display per vendor — raw snapshot + fetch metadata for native
/// panel rendering, or an error message when the fetch failed.
///
/// `Ready` is boxed because the snapshot is much larger than the other two
/// variants (silences `clippy::large_enum_variant`).
#[derive(Debug, Clone)]
pub enum TabState {
    Loading,
    Ready(Box<ReadyTab>),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ReadyTab {
    pub snapshot: crate::usage::VendorSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    /// Absolute moment the cache was written (i.e. the API response landed).
    /// Snapshotted once at TabState build time so the rendered "Updated …"
    /// timestamp stays stable across redraws instead of drifting with the
    /// passing wall clock.
    pub fetched_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Identity of one TUI tab. Usually a whole vendor; Claude and OpenRouter can
/// also name a configured account. `account: None` is a plain vendor tab or
/// that vendor's default account.
/// `desktop` marks an account whose usage comes from the Claude Desktop app's
/// own token rather than a `claude` CLI credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TabId {
    pub vendor: VendorId,
    pub account: Option<String>,
    pub desktop: bool,
}

impl TabId {
    /// A plain vendor tab (default account for Anthropic).
    pub fn vendor(vendor: VendorId) -> Self {
        Self {
            vendor,
            account: None,
            desktop: false,
        }
    }

    /// A named Anthropic account tab (`[[anthropic.accounts]]` label).
    pub fn account(label: impl Into<String>) -> Self {
        Self::account_for(VendorId::Anthropic, label)
    }

    /// A named account for a vendor that supports account arrays.
    pub fn account_for(vendor: VendorId, label: impl Into<String>) -> Self {
        Self {
            vendor,
            account: Some(label.into()),
            desktop: false,
        }
    }

    /// An Anthropic account whose usage is read from the Claude Desktop app's
    /// own token store (a saved `~/.claude-acc/profiles/<label>` account).
    pub fn desktop_account(label: impl Into<String>) -> Self {
        Self {
            vendor: VendorId::Anthropic,
            account: Some(label.into()),
            desktop: true,
        }
    }
}

/// Expand enabled vendors into the tab list. Claude and OpenRouter yield their
/// default account followed by configured named accounts; every other vendor
/// is a single tab. With no extra accounts the result equals
/// `config.enabled_vendors()`, preserving the historical tab set and order.
///
/// Config-only and pure — no Desktop profiles. Production uses
/// [`tabs_with_desktop`]; this stays for the hermetic unit tests and any caller
/// that only wants configured accounts.
pub fn tabs_from_config(config: &Config) -> Vec<TabId> {
    build_tabs(config, &[], &DefaultCreds::detect(config))
}

/// The production aggregate-view tab list: configured accounts plus every saved
/// Claude Desktop profile that has usable credentials. Desktop discovery is
/// best-effort and macOS-only; anywhere else this equals [`tabs_from_config`].
pub fn tabs_with_desktop(config: &Config) -> Vec<TabId> {
    build_tabs(
        config,
        &desktop_profile_labels(config),
        &DefaultCreds::detect(config),
    )
}

/// Core expansion, parameterized on the Desktop account labels so it stays pure
/// and unit-testable. Desktop accounts follow the CLI accounts and count toward
/// "Anthropic has accounts" for the default-tab suppression.
///
/// In aggregate views, a label present in both a `[[anthropic.accounts]]` CLI
/// entry and a Desktop profile is sourced from **Desktop**, and the CLI entry is
/// dropped. The same account in two stores means two of them refreshing one
/// rotating refresh token — each rotation invalidates the other's copy — and
/// the CLI copy can even refresh to a stale/wrong identity that still
/// authenticates but reports another account's (often zero) usage, which no
/// credential-health check can catch. The app-maintained Desktop token is the
/// one source that avoids both the rotation war and that silent misattribution.
/// Whether each vendor's DEFAULT section carries a credential, for the tab
/// builder. An unconfigured default is not a tab worth showing once
/// numbered accounts exist — the settings panel's Add flow puts keys ON
/// accounts, so a keyless default next to working accounts is exactly the
/// dead "GLM / Kimi" node that clutters the tab strip. Injectable so tests
/// stay hermetic (no ambient env vars, no real `~/.kimi-code`).
pub(crate) struct DefaultCreds<'a> {
    pub env_set: &'a dyn Fn(&str) -> bool,
    pub kimi_cli_login: bool,
}

impl DefaultCreds<'_> {
    /// The production detector (config-aware for the kimi login override).
    pub fn detect(config: &Config) -> DefaultCreds<'static> {
        DefaultCreds {
            env_set: &|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()),
            kimi_cli_login: crate::kimi::cli_login_present(&config.kimi),
        }
    }
}

/// Whether a key-account vendor's default section is configured: an inline
/// key, its env var, or (kimi only) the CLI's own OAuth login.
fn default_credential_present(config: &Config, vendor: VendorId, creds: &DefaultCreds) -> bool {
    // Not a config-key vendor — its credential lives elsewhere (OAuth
    // login, local product state), so it always counts as present. Note an
    // ABSENT inline key is still `None` here without the env lookup: only
    // both being absent marks a non-key vendor.
    if config.api_key_env_name(vendor).is_none() {
        return true;
    }
    if vendor == VendorId::Kimi && creds.kimi_cli_login {
        return true;
    }
    config
        .inline_api_key(vendor)
        .is_some_and(|key| !key.is_empty())
        || config
            .api_key_env_name(vendor)
            .is_some_and(|env| (creds.env_set)(env))
}

fn build_tabs(config: &Config, desktop_labels: &[String], creds: &DefaultCreds) -> Vec<TabId> {
    let desktop_set: HashSet<&str> = desktop_labels.iter().map(String::as_str).collect();
    let mut tabs = Vec::new();
    for vendor in config.enabled_vendors() {
        if vendor == VendorId::Anthropic {
            let accounts: Vec<_> = config
                .anthropic
                .all_accounts()
                .into_iter()
                .filter(|a| !desktop_set.contains(a.label.as_str()))
                .collect();
            // The default (unnamed) Claude tab is suppressible once every
            // account is named — but never when it would leave Anthropic with
            // no tab at all. Desktop accounts count as named accounts here.
            if config.anthropic.show_default_account
                || (accounts.is_empty() && desktop_labels.is_empty())
            {
                tabs.push(TabId::vendor(vendor));
            }
            for acct in accounts {
                tabs.push(TabId::account(acct.label));
            }
            for label in desktop_labels {
                tabs.push(TabId::desktop_account(label.clone()));
            }
        } else if vendor == VendorId::Zai {
            // A keyless default next to numbered accounts is a dead node —
            // hide it unless it actually carries a credential.
            if config.zai.accounts.is_empty()
                || (config.zai.show_default_account
                    && default_credential_present(config, vendor, creds))
            {
                tabs.push(TabId::vendor(vendor));
            }
            for account in &config.zai.accounts {
                tabs.push(TabId::account_for(vendor, account.label.clone()));
            }
        } else if let Some(accounts) = config.key_accounts(vendor) {
            // Every key-based vendor expands the same way: the default tab
            // only when it is credentialed (or nothing else exists), then
            // one tab per numbered account.
            if accounts.is_empty()
                || (config.key_show_default(vendor)
                    && default_credential_present(config, vendor, creds))
            {
                tabs.push(TabId::vendor(vendor));
            }
            for account in accounts {
                tabs.push(TabId::account_for(vendor, account.label.clone()));
            }
        } else {
            tabs.push(TabId::vendor(vendor));
        }
    }
    tabs
}

/// Labels of saved Claude Desktop profiles with usable credentials. macOS-only
/// (elsewhere there is no Desktop app); best-effort, so an unreadable profile
/// store just yields none rather than failing the whole tab list.
#[cfg(target_os = "macos")]
fn desktop_profile_labels(config: &Config) -> Vec<String> {
    let Ok(paths) = crate::claude_desktop::Paths::resolve(&config.anthropic) else {
        return Vec::new();
    };
    if !paths.available() {
        return Vec::new();
    }
    crate::claude_desktop::load_profiles(&paths.profiles_dir)
        .into_iter()
        .filter(|p| p.has_credentials)
        .map(|p| p.label)
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn desktop_profile_labels(_config: &Config) -> Vec<String> {
    Vec::new()
}

/// Fetch and render one tab — returns a `TabState`.
pub async fn refresh_one(client: &Client, config: &Config, tab: &TabId) -> TabState {
    match build_outcome(client, config, tab).await {
        Ok(outcome) => {
            // Resolve the cache age (a duration from "now" at fetch time) into an
            // absolute instant ONCE. Without this, sections_for would recompute
            // `Utc::now() - cache_age` on every draw and the displayed time would
            // tick upward in real time instead of holding at the last refresh.
            let now = Utc::now();
            let fetched_at = outcome
                .cache_age
                .map(|age| now - chrono::Duration::from_std(age).unwrap_or_default());
            TabState::Ready(Box::new(ReadyTab {
                snapshot: outcome.snapshot,
                stale: outcome.stale,
                last_error: outcome.last_error.map(|(code, message)| {
                    (code, crate::display::sanitize_untrusted_field(&message))
                }),
                fetched_at,
            }))
        }
        Err(e) => TabState::Error(crate::display::sanitize_untrusted_field(&e.user_message())),
    }
}

/// Resolve the API key for a key-account vendor's tab: the numbered
/// account's key when the tab addresses one, the section default otherwise.
fn key_account_api_key(
    config: &Config,
    vendor: VendorId,
    display: &str,
    section: &str,
    default_env: &str,
    default_key: Option<&str>,
    tab: &TabId,
) -> Result<String> {
    match tab.account.as_deref() {
        Some(label) => crate::config::resolve_key_account(
            display,
            section,
            config.key_accounts(vendor).unwrap_or_default(),
            label,
        ),
        None => crate::config::resolve_api_key(display, default_env, default_key),
    }
}

/// Cache for a key-account vendor's tab: account-scoped when addressed, the
/// vendor default otherwise.
fn key_account_cache(slug: &str, tab: &TabId) -> Result<crate::cache::Cache> {
    match tab.account.as_deref() {
        Some(label) => crate::cache::Cache::for_vendor_account(slug, label),
        None => crate::cache::Cache::for_vendor(slug),
    }
}

async fn build_outcome(client: &Client, config: &Config, tab: &TabId) -> Result<VendorOutcome> {
    match tab.vendor {
        VendorId::Anthropic => {
            // A named account resolves to its own file + `anthropic/<label>`
            // cache, shared with the widget via `account_target` (#14/#17).
            // The default tab keeps the pre-existing resolution: config
            // `credentials_path` is an explicit strict read, and only the
            // platform default gets the macOS Keychain fallback.
            let (creds_target, cache) = match tab.account.as_deref() {
                Some(label) if tab.desktop => {
                    crate::anthropic::desktop_creds::account_target(config, label)?
                }
                Some(label) => config.anthropic.account_target(label)?,
                None => {
                    let target = match config.anthropic.credentials_path.clone() {
                        Some(p) => crate::anthropic::creds::CredsTarget::Explicit(p),
                        None => crate::anthropic::creds::CredsTarget::Default(
                            crate::anthropic::creds::default_path().unwrap_or_default(),
                        ),
                    };
                    (target, crate::cache::Cache::for_vendor("anthropic")?)
                }
            };
            let endpoints = crate::anthropic::fetch::Endpoints::default();
            let outcome = crate::anthropic::fetch_snapshot(
                client,
                &creds_target,
                &cache,
                &endpoints,
                crate::cache::GENTLE_TTL,
            )
            .await?;
            Ok(crate::vendor::VendorOutcome {
                snapshot: crate::usage::VendorSnapshot::Anthropic(outcome.snapshot),
                stale: outcome.stale,
                last_error: outcome.last_error,
                cache_age: outcome.cache_age,
            })
        }
        VendorId::AnthropicApi => {
            // Numbered accounts carry their own Admin key and MAY carry
            // their own monthly limit (each key is its own organization);
            // the section limit stays the default account's.
            let key = key_account_api_key(
                config,
                VendorId::AnthropicApi,
                "Anthropic_API",
                "anthropic_api",
                &config.anthropic_api.api_key_env,
                config.anthropic_api.api_key.as_deref(),
                tab,
            )?;
            let cache = key_account_cache("anthropic_api", tab)?;
            let limit = match tab.account.as_deref().and_then(|label| {
                config
                    .key_accounts(VendorId::AnthropicApi)
                    .and_then(|accounts| accounts.iter().find(|a| a.label == label))
            }) {
                Some(account) => account.monthly_limit.or(config.anthropic_api.monthly_limit),
                None => config.anthropic_api.monthly_limit,
            };
            let endpoints = crate::anthropic_api::fetch::Endpoints::default();
            let outcome = crate::anthropic_api::fetch_snapshot(
                client,
                &key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                limit,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Openrouter => {
            let api_key = config.openrouter.resolve_api_key(tab.account.as_deref())?;
            let cache = match tab.account.as_deref() {
                Some(label) => crate::cache::Cache::for_vendor_account("openrouter", label)?,
                None => crate::cache::Cache::for_vendor("openrouter")?,
            };
            let endpoints = crate::openrouter::fetch::Endpoints::default();
            let outcome = crate::openrouter::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Zai => {
            // The default tab keeps the historical personal-on-api.z.ai
            // resolution; named accounts carry their own billing type, site,
            // and team scope, and get an isolated `zai/<label>` cache.
            let plan = config.zai.resolve_account(tab.account.as_deref())?;
            let cache = match tab.account.as_deref() {
                Some(label) => crate::cache::Cache::for_vendor_account("zai", label)?,
                None => crate::cache::Cache::for_vendor("zai")?,
            };
            let endpoints = crate::zai::fetch::Endpoints::for_site(plan.site);
            let outcome = crate::zai::fetch::fetch_snapshot_plan(
                client,
                &plan,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                Utc::now(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Openai => {
            let cache = crate::cache::Cache::for_vendor("openai")?;
            let creds_path = config
                .openai
                .codex_auth_path
                .clone()
                .unwrap_or_else(|| crate::openai::creds::default_path().unwrap_or_default());
            let endpoints = crate::openai::fetch::Endpoints::default();
            let outcome = crate::openai::fetch_snapshot(
                client,
                &creds_path,
                &cache,
                &endpoints,
                crate::cache::GENTLE_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::OpenaiApi => {
            // Same shape as AnthropicApi: numbered accounts carry their own
            // Admin key and MAY their own monthly limit; GENTLE_TTL because
            // the costs data is daily-bucket reporting.
            let key = key_account_api_key(
                config,
                VendorId::OpenaiApi,
                "OpenAI API",
                "openai_api",
                &config.openai_api.api_key_env,
                config.openai_api.api_key.as_deref(),
                tab,
            )?;
            let cache = key_account_cache("openai_api", tab)?;
            let limit = match tab.account.as_deref().and_then(|label| {
                config
                    .key_accounts(VendorId::OpenaiApi)
                    .and_then(|accounts| accounts.iter().find(|a| a.label == label))
            }) {
                Some(account) => account.monthly_limit.or(config.openai_api.monthly_limit),
                None => config.openai_api.monthly_limit,
            };
            let endpoints = crate::openai_api::fetch::Endpoints::default();
            let outcome = crate::openai_api::fetch_snapshot(
                client,
                &key,
                &cache,
                &endpoints,
                crate::cache::GENTLE_TTL,
                limit,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Deepseek => {
            let api_key = crate::config::resolve_api_key(
                "DeepSeek",
                &config.deepseek.api_key_env,
                config.deepseek.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("deepseek")?;
            let endpoints = crate::deepseek::fetch::Endpoints::default();
            let outcome =
                crate::deepseek::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Kimi => {
            // Named accounts are API-key based and fetch through an isolated
            // `kimi/<label>` cache; the default tab keeps the CLI-login or
            // API-key resolution unchanged.
            let cache = match tab.account.as_deref() {
                Some(label) => crate::cache::Cache::for_vendor_account("kimi", label)?,
                None => crate::cache::Cache::for_vendor("kimi")?,
            };
            let (auth, endpoints) =
                crate::kimi::resolve_auth_for(&config.kimi, tab.account.as_deref())?;
            let outcome = crate::kimi::fetch::fetch_snapshot_with_auth(
                client,
                &auth,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Kilo => {
            let api_key = crate::config::resolve_api_key(
                "Kilo",
                &config.kilo.api_key_env,
                config.kilo.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("kilo")?;
            let endpoints = crate::kilo::fetch::Endpoints::default();
            let outcome = crate::kilo::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.kilo.organization_id.as_deref(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Novita => {
            let api_key = crate::config::resolve_api_key(
                "Novita",
                &config.novita.api_key_env,
                config.novita.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("novita")?;
            let endpoints = crate::novita::fetch::Endpoints::default();
            let outcome =
                crate::novita::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Moonshot => {
            let api_key = crate::config::resolve_api_key(
                "Moonshot",
                &config.moonshot.api_key_env,
                config.moonshot.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("moonshot")?;
            let (endpoints, currency) =
                crate::moonshot::fetch::Endpoints::for_region(&config.moonshot.region);
            let outcome = crate::moonshot::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                currency,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Grok => {
            let api_key = key_account_api_key(
                config,
                VendorId::Grok,
                "Grok",
                "grok",
                &config.grok.api_key_env,
                config.grok.api_key.as_deref(),
                tab,
            )?;
            let cache = key_account_cache("grok", tab)?;
            let endpoints = crate::grok::fetch::Endpoints::default();
            let outcome = crate::grok::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.grok.team_id.as_deref(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Supergrok => {
            let cache = crate::cache::Cache::for_vendor("supergrok")?;
            let scope_paths = crate::supergrok::scope::ScopePaths::with_overrides(
                config.supergrok.auth_path.as_deref(),
                config.supergrok.config_path.as_deref(),
            )?;
            let outcome = crate::supergrok::fetch_snapshot(
                &config.supergrok.grok_binary,
                &scope_paths,
                &cache,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Antigravity => {
            // No credentials: the local Antigravity server is the source.
            let cache = crate::cache::Cache::for_vendor("antigravity")?;
            let outcome = crate::antigravity::fetch_snapshot(client, &cache, DEFAULT_TTL).await?;
            Ok(outcome.into())
        }
        VendorId::Minimax => {
            let api_key = crate::config::resolve_api_key(
                "MiniMax",
                &config.minimax.api_key_env,
                config.minimax.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("minimax")?;
            let endpoints = crate::minimax::fetch::Endpoints::for_region(&config.minimax.region);
            let outcome =
                crate::minimax::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Cursor => {
            let cache = crate::cache::Cache::for_vendor("cursor")?;
            let db_path = config
                .cursor
                .db_path
                .clone()
                .map(Ok)
                .unwrap_or_else(crate::cursor::db::default_db_path)?;
            let agent_auth_path = config
                .cursor
                .agent_auth_path
                .clone()
                .map(Ok)
                .unwrap_or_else(crate::cursor::db::default_agent_auth_path)?;
            let endpoints = crate::cursor::fetch::Endpoints::default();
            let outcome = crate::cursor::fetch_snapshot(
                client,
                &db_path,
                &agent_auth_path,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Kiro => {
            let cache = crate::cache::Cache::for_vendor("kiro")?;
            let db_path = config
                .kiro
                .db_path
                .clone()
                .map(Ok)
                .unwrap_or_else(crate::kiro::db::default_db_path)?;
            let outcome =
                crate::kiro::fetch_snapshot(client, &db_path, &cache, DEFAULT_TTL).await?;
            Ok(outcome.into())
        }
        VendorId::OpenCodeGo => {
            let api_key = crate::config::resolve_api_key(
                "OpenCode Go",
                &config.opencode_go.api_key_env,
                config.opencode_go.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("opencode-go")?;
            let endpoints = crate::opencode_go::fetch::Endpoints::default();
            let outcome = crate::opencode_go::fetch::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
    }
}

/// Convenience for the watch-driven binary: how long to wait between
/// automatic refreshes.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Gap between successive Anthropic fetches at refresh time. Every Anthropic
/// tab (the default account and each named/discovered account) hits the same
/// `/api/oauth/usage` + token-refresh endpoints, which rate-limit a burst of
/// simultaneous requests from one client — so with several accounts the TUI
/// would fire them all at once and some would come back `429`. Spacing them
/// out keeps every account refreshing politely.
pub const ANTHROPIC_REFRESH_STAGGER: Duration = Duration::from_millis(800);

/// Per-tab startup delay for one `spawn_all` pass. Only Anthropic tabs are
/// staggered (they share the rate-limited endpoint and multiply with accounts);
/// every other vendor hits its own endpoint and starts immediately. The first
/// Anthropic tab also starts immediately; each subsequent one waits one more
/// `step`. Pure and position-based so it is unit-testable.
pub fn refresh_stagger(tabs: &[TabId], step: Duration) -> Vec<Duration> {
    let mut anthropic_seen: u32 = 0;
    tabs.iter()
        .map(|tab| {
            if tab.vendor == VendorId::Anthropic {
                let delay = step * anthropic_seen;
                anthropic_seen += 1;
                delay
            } else {
                Duration::ZERO
            }
        })
        .collect()
}

/// One row of the panel body. Vendors emit a `Vec<Section>`; the TUI
/// renderer turns them into terminal widgets.
pub enum Section {
    /// Title row at the top. `left` is the plan/vendor label (accent-colored,
    /// bold); `right` is an optional right-aligned annotation, used for the
    /// "Updated HH:MM:SS" timestamp so it shares the title row instead of
    /// taking a separate body row + duplicating the global footer's clock.
    Title { left: String, right: Option<String> },
    /// A metric: label + gauge + value annotation + dim footnote.
    Metric {
        label: String,
        pct: u16,
        severity: PaceSeverity,
        value_label: String,
        footnote: String,
    },
    /// Free-form key/value text line.
    Text { label: String, value: String },
    /// A label followed by a multi-line dim block (no gauge).
    Block { label: String, body: Vec<String> },
    /// Visual spacer (one blank row).
    Spacer,
}

/// Internal metadata carried alongside a public [`Section`]. Keeping this
/// wrapper private to the crate lets machine-readable frontends receive
/// absolute reset timestamps without adding a source-breaking field to the
/// public `Section::Metric` variant.
pub(crate) struct SectionProjection {
    pub section: Section,
    pub reset_at: Option<DateTime<Utc>>,
    /// Which rolling window a metric row describes (`session` ≈ the 5h
    /// window, `weekly` the 7d one), classified here — the single place —
    /// off the label the vendor itself supplied. Consumed by the report
    /// JSON and the aggregate bar so no frontend re-derives it.
    pub window: Option<&'static str>,
}

/// Classify a metric row's window from its Rust-generated label
/// ("Session (5h)", "Rolling window (5h)", "Weekly quota", …).
pub(crate) fn metric_window(label: &str) -> Option<&'static str> {
    let lower = label.to_lowercase();
    if lower.contains("session") || lower.contains("rolling") || lower.contains("5h") {
        Some("session")
    } else if lower.contains("week") {
        Some("weekly")
    } else {
        None
    }
}

struct SectionBuilder(Vec<SectionProjection>);

impl SectionBuilder {
    fn new(sections: Vec<Section>) -> Self {
        Self(
            sections
                .into_iter()
                .map(|section| {
                    assert!(
                        !matches!(section, Section::Metric { .. }),
                        "metric sections must declare reset metadata with push_metric"
                    );
                    SectionProjection {
                        section,
                        reset_at: None,
                        window: None,
                    }
                })
                .collect(),
        )
    }

    fn push(&mut self, section: Section) {
        assert!(
            !matches!(section, Section::Metric { .. }),
            "metric sections must declare reset metadata with push_metric"
        );
        self.0.push(SectionProjection {
            section,
            reset_at: None,
            window: None,
        });
    }

    fn push_metric(&mut self, section: Section, reset_at: Option<DateTime<Utc>>) {
        assert!(matches!(section, Section::Metric { .. }));
        let window = match &section {
            Section::Metric { label, .. } => metric_window(label),
            _ => None,
        };
        self.0.push(SectionProjection {
            section,
            reset_at,
            window,
        });
    }
}

/// Rich projection used by machine-readable frontends. The TUI continues to
/// expose the source-compatible [`sections_for`] result above.
pub(crate) fn sections_with_metadata_for(
    tab: &TabState,
    now: DateTime<Utc>,
    pace_tolerance: u32,
) -> Vec<SectionProjection> {
    let mut sections = match tab {
        TabState::Loading => SectionBuilder::new(vec![
            Section::Spacer,
            Section::Text {
                label: "".into(),
                value: "  Loading…".into(),
            },
        ]),
        TabState::Error(e) => SectionBuilder::new(vec![
            Section::Spacer,
            Section::Text {
                label: "Error".into(),
                value: e.clone(),
            },
            Section::Spacer,
            Section::Text {
                label: "".into(),
                value: "Press `s` to configure, `r` to retry, `q` to quit.".into(),
            },
        ]),
        TabState::Ready(r) => {
            let snapshot = &r.snapshot;
            let last_error = &r.last_error;
            let mut sections = match snapshot {
                VendorSnapshot::Anthropic(s) => anthropic_sections(s, now, pace_tolerance),
                VendorSnapshot::AnthropicApi(s) => anthropic_api_sections(s),
                VendorSnapshot::OpenaiApi(s) => openai_api_sections(s),
                VendorSnapshot::Openai(s) => openai_sections(s, now, pace_tolerance),
                VendorSnapshot::Zai(s) => zai_sections(s, now),
                VendorSnapshot::Openrouter(s) => openrouter_sections(s),
                VendorSnapshot::Deepseek(s) => deepseek_sections(s),
                VendorSnapshot::Kimi(s) => kimi_sections(s, now, pace_tolerance),
                VendorSnapshot::Kilo(s) => kilo_sections(s),
                VendorSnapshot::Novita(s) => novita_sections(s),
                VendorSnapshot::Moonshot(s) => moonshot_sections(s),
                VendorSnapshot::Grok(s) => grok_sections(s),
                VendorSnapshot::SuperGrok(s) => supergrok_sections(s, now),
                VendorSnapshot::Antigravity(s) => antigravity_sections(s, now),
                VendorSnapshot::Cursor(s) => cursor_sections(s, now),
                VendorSnapshot::Minimax(s) => minimax_sections(s, now, pace_tolerance),
                VendorSnapshot::Kiro(s) => kiro_sections(s, now),
                VendorSnapshot::OpenCodeGo(s) => opencode_go_sections(s, now),
            };
            // Inject the (already-absolute) fetched-at instant into the title
            // row, right-aligned. Pre-snapshotted in app::refresh_one so it
            // doesn't drift between redraws.
            let updated = match r.fetched_at {
                Some(at) => format!("Updated {}", local_time_hms(at)),
                None => "Updated —".to_string(),
            };
            if let Some(SectionProjection {
                section: Section::Title { right, .. },
                ..
            }) = sections.0.first_mut()
            {
                *right = Some(updated);
            }
            // Error footer (when present) still lives in the body.
            if let Some((label, msg)) = warning_label(snapshot, last_error) {
                sections.push(Section::Spacer);
                sections.push(Section::Text { label, value: msg });
            }
            sections
        }
    };
    for projected in &mut sections.0 {
        sanitize_section(&mut projected.section);
    }
    sections.0
}

/// Sanitize at the final projection boundary so every vendor field, cached
/// diagnostic, and fetch error is inert before the TUI writes it to a terminal.
fn sanitize_section(section: &mut Section) {
    let clean = |value: &mut String| {
        *value = crate::display::sanitize_untrusted_field(value);
    };
    match section {
        Section::Title { left, right } => {
            clean(left);
            if let Some(right) = right {
                clean(right);
            }
        }
        Section::Metric {
            label,
            value_label,
            footnote,
            ..
        } => {
            clean(label);
            clean(value_label);
            clean(footnote);
        }
        Section::Text { label, value } => {
            clean(label);
            clean(value);
        }
        Section::Block { label, body } => {
            clean(label);
            for line in body {
                clean(line);
            }
        }
        Section::Spacer => {}
    }
}

/// Translate cache diagnostics at the presentation boundary. Cache files keep
/// their established `(u16, String)` form: only non-zero codes are HTTP, while
/// Kimi's stable schema marker identifies its code-zero schema warning.
pub(crate) fn warning_label(
    snapshot: &VendorSnapshot,
    last_error: &Option<(u16, String)>,
) -> Option<(String, String)> {
    let (code, message) = last_error.as_ref()?;
    if *code != 0 {
        return Some((format!("HTTP {code}"), message.clone()));
    }
    if message.is_empty() {
        return None;
    }
    let label = if matches!(snapshot, VendorSnapshot::Kimi(_))
        && matches!(
            crate::kimi::vendor::warning_kind(*code, message),
            crate::kimi::vendor::WarningKind::SchemaDrift
        ) {
        "Kimi API schema drift"
    } else {
        "Warning"
    };
    // The stable marker is already the schema-warning label. Keep the label
    // visible but do not repeat that sentinel as a redundant body value.
    let value = if label == message {
        String::new()
    } else {
        message.clone()
    };
    Some((label.into(), value))
}

fn anthropic_api_sections(s: &crate::usage::AnthropicApiSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: "Anthropic API".into(),
        right: None,
    }]);
    match (s.limit.filter(|l| *l > 0.0), s.pct()) {
        (Some(limit), Some(pct)) => {
            let p = pct.clamp(0, 100) as u16;
            v.push_metric(
                Section::Metric {
                    label: "Spend (mo)".into(),
                    pct: p,
                    severity: severity_for(pct),
                    value_label: format!("{} of ${:.0}", usd(s.spent), limit),
                    footnote: format!("{pct}% of monthly limit"),
                },
                None,
            );
        }
        _ => {
            v.push(Section::Text {
                label: "Spend (mo)".into(),
                value: usd(s.spent),
            });
        }
    }
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "".into(),
        value: "Month-to-date cost via the Admin usage API.".into(),
    });
    v.push(Section::Text {
        label: "".into(),
        value: "Prepaid credit balance is Console-only (no API).".into(),
    });
    v.push(Section::Text {
        label: "".into(),
        value: "Excludes Priority Tier cost (not reported by this API).".into(),
    });
    v
}

fn openai_api_sections(s: &crate::usage::OpenAiApiSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: "OpenAI API".into(),
        right: None,
    }]);
    match (s.limit.filter(|l| *l > 0.0), s.pct()) {
        (Some(limit), Some(pct)) => {
            let p = pct.clamp(0, 100) as u16;
            v.push_metric(
                Section::Metric {
                    label: "Spend (30d)".into(),
                    pct: p,
                    severity: severity_for(pct),
                    value_label: format!("{} of ${:.0}", usd(s.spent), limit),
                    footnote: format!("{pct}% of monthly limit"),
                },
                None,
            );
        }
        _ => {
            v.push(Section::Text {
                label: "Spend (30d)".into(),
                value: usd(s.spent),
            });
        }
    }
    if !s.top_items.is_empty() {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Top line items".into(),
            body: s
                .top_items
                .iter()
                .take(6)
                .map(|(name, value)| format!("{name} — {}", usd(*value)))
                .collect(),
        });
    }
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "".into(),
        value: "Trailing-30-day billed cost via the Costs API.".into(),
    });
    v.push(Section::Text {
        label: "".into(),
        value: "Admin API key required; project sk- keys are rejected.".into(),
    });
    v.push(Section::Text {
        label: "".into(),
        value: "Spend, not balance — the Admin API has no balance endpoint.".into(),
    });
    v
}

fn anthropic_sections(
    s: &crate::usage::AnthropicSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: format!("Claude {}", s.plan),
        right: None,
    }]);

    push_window(&mut v, "Session (5h)", &s.session, now, tol, true);
    push_window(&mut v, "Weekly (7d)", &s.weekly, now, tol, true);
    if let Some(w) = &s.sonnet {
        push_window(&mut v, "Sonnet only", w, now, tol, false);
    }
    for sw in &s.scoped {
        push_window(
            &mut v,
            &format!("{} (7d)", sw.label),
            &sw.window,
            now,
            tol,
            false,
        );
    }
    if let Some(e) = &s.extra {
        v.push(Section::Spacer);
        let pct = e.percent().clamp(0, 100) as u16;
        // An uncapped plan (`monthly_limit: null`) has spend but no
        // denominator: show the amount alone rather than "of $0.00" or a
        // percentage nobody can vouch for (#30).
        let (value_label, footnote) = match e.fmt_limit() {
            Some(l) => (
                format!("{} of {}", e.fmt_spent(), l),
                format!("{pct}% of monthly limit consumed"),
            ),
            None => (e.fmt_spent(), "no monthly limit reported".to_string()),
        };
        v.push_metric(
            Section::Metric {
                label: "Extra usage".into(),
                pct,
                severity: severity_for(pct as i32),
                value_label,
                footnote,
            },
            None,
        );
    }
    v
}

fn openai_sections(
    s: &crate::usage::OpenAiSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    if let Some(session) = &s.session {
        push_window(&mut v, "Codex 5h", session, now, tol, true);
    }
    if let Some(weekly) = &s.weekly {
        push_window(&mut v, "Codex weekly", weekly, now, tol, true);
    }
    if s.session.is_none() && s.weekly.is_none() {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "".into(),
            value: "  no usage windows reported".into(),
        });
    }
    if let Some(cr) = &s.code_review {
        push_window(&mut v, "Code review", cr, now, tol, false);
    }
    if let Some(c) = &s.credits {
        v.push(Section::Spacer);
        let balance = if c.unlimited {
            "unlimited".into()
        } else {
            c.balance.clone()
        };
        let mut body = vec![format!("balance: {}", balance)];
        if let Some((lo, hi)) = c.approx_local_messages {
            body.push(format!("≈ {lo}-{hi} local messages"));
        }
        if let Some((lo, hi)) = c.approx_cloud_messages {
            body.push(format!("≈ {lo}-{hi} cloud messages"));
        }
        v.push(Section::Block {
            label: "Credits".into(),
            body,
        });
    }
    v
}

fn zai_sections(s: &crate::usage::ZaiSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    if let Some(w) = &s.session {
        push_window(
            &mut v,
            "Session (5h)",
            w,
            now,
            crate::pacing::DEFAULT_TOLERANCE,
            false,
        );
    }
    if let Some(w) = &s.weekly {
        push_window(
            &mut v,
            "Weekly",
            w,
            now,
            crate::pacing::DEFAULT_TOLERANCE,
            false,
        );
    }
    if let Some(w) = &s.mcp {
        push_window(
            &mut v,
            "MCP tools (monthly)",
            w,
            now,
            crate::pacing::DEFAULT_TOLERANCE,
            false,
        );
    }
    if s.session.is_none() && s.weekly.is_none() && s.mcp.is_none() {
        v.push(Section::Spacer);
        match s.usage_stats {
            // Pay-as-you-go key: the 7-day aggregate is the whole report.
            Some(stats) => {
                v.push(Section::Text {
                    label: "7-day usage".into(),
                    value: format!(
                        "{} prompts · {} tokens",
                        compact_count(stats.prompts),
                        compact_count(stats.tokens)
                    ),
                });
            }
            None => {
                v.push(Section::Text {
                    label: "".into(),
                    value: "  no usage windows reported".into(),
                });
            }
        }
    }
    v
}

/// Compact `1.2k` / `3.4M` rendering for the Z.AI 7-day counters (raw token
/// counts run to nine digits and blow out the panel width).
fn compact_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn openrouter_sections(s: &crate::usage::OpenRouterSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.label.clone(),
        right: None,
    }]);
    let pct = s.consumed_pct().clamp(0, 100) as u16;
    v.push(Section::Spacer);
    v.push_metric(
        Section::Metric {
            label: "Credit balance".into(),
            pct,
            // One severity policy for every frontend: this value is what the
            // Omarchy, GNOME and KDE panels colour their row with, so it has to
            // agree with the Waybar tooltip about what "in debt" looks like.
            severity: crate::openrouter::vendor::severity(s),
            value_label: usd(s.balance()),
            footnote: format!(
                "{} of {} used ({pct}%)",
                usd(s.total_usage),
                usd(s.total_credits)
            ),
        },
        None,
    );
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "Usage by period".into(),
        body: vec![format!(
            "today ${:.2} · week ${:.2} · month ${:.2}",
            s.usage_daily, s.usage_weekly, s.usage_monthly
        )],
    });
    if let (Some(limit), Some(rem)) = (s.limit, s.limit_remaining) {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Per-key limit".into(),
            body: vec![format!("{} of {} remaining", usd(rem), usd(limit))],
        });
    }
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "Tier".into(),
        body: vec![if s.is_free_tier {
            "free tier".into()
        } else {
            "paid tier".into()
        }],
    });
    v
}

/// Antigravity holds two independent pools (Gemini, Claude & GPT OSS), each
/// with a 5-hour and a weekly window. Grouped by window type so the two pools
/// sit side by side, matching the GNOME dropdown.
fn antigravity_sections(
    s: &crate::usage::AntigravitySnapshot,
    now: DateTime<Utc>,
) -> SectionBuilder {
    use crate::antigravity::vendor::{GROUP_PRIMARY, GROUP_THIRD_PARTY};

    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    for (heading, primary, third_party) in [
        ("Session", &s.session, s.third_party_session.as_ref()),
        ("Weekly", &s.weekly, s.third_party_weekly.as_ref()),
    ] {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: heading.into(),
            value: String::new(),
        });
        push_window(
            &mut v,
            GROUP_PRIMARY,
            primary,
            now,
            crate::pacing::DEFAULT_TOLERANCE,
            false,
        );
        if let Some(w) = third_party {
            push_window(
                &mut v,
                GROUP_THIRD_PARTY,
                w,
                now,
                crate::pacing::DEFAULT_TOLERANCE,
                false,
            );
        }
    }
    v
}

fn cursor_sections(s: &crate::usage::CursorSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: format!("Cursor {}", s.plan),
        right: None,
    }]);
    if s.unlimited {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "Plan".into(),
            value: "Unlimited — pools don't cap".into(),
        });
    } else {
        // Two included-usage pools, mirroring the dashboard's two bars.
        v.push(Section::Spacer);
        v.push_metric(
            Section::Metric {
                label: "Cursor Models".into(),
                pct: s.auto_pct.clamp(0, 100) as u16,
                severity: severity_for(s.auto_pct),
                value_label: format!("{}%", s.auto_pct),
                footnote: "Auto + Composer".into(),
            },
            s.reset_at,
        );
        v.push(Section::Spacer);
        v.push_metric(
            Section::Metric {
                label: "Other Models".into(),
                pct: s.api_pct.clamp(0, 100) as u16,
                severity: severity_for(s.api_pct),
                value_label: format!("{}%", s.api_pct),
                footnote: format!(
                    "Named / API models · on-demand {}",
                    if s.on_demand_enabled { "on" } else { "off" }
                ),
            },
            s.reset_at,
        );
    }
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Resets".into(),
        value: countdown::format(s.reset_at, now),
    });
    v
}

fn opencode_go_sections(
    s: &crate::opencode_go::types::Usage,
    now: DateTime<Utc>,
) -> SectionBuilder {
    let mut sections = SectionBuilder::new(vec![Section::Title {
        left: "OpenCode Go".into(),
        right: None,
    }]);
    for (label, window) in [
        ("Rolling", s.rolling.as_ref()),
        ("Weekly", s.weekly.as_ref()),
        ("Monthly", s.monthly.as_ref()),
    ] {
        if let Some(window) = window {
            let pct = window.percent.round().clamp(0.0, 100.0) as i32;
            sections.push_metric(
                Section::Metric {
                    label: label.into(),
                    pct: pct as u16,
                    severity: severity_for(pct),
                    value_label: format!("{pct}%"),
                    footnote: String::new(),
                },
                Some(window.resets_at),
            );
            sections.push(Section::Text {
                label: "Resets".into(),
                value: countdown::format(Some(window.resets_at), now),
            });
        }
    }
    sections
}

/// Kiro has a single credit pool, so the panel is a single metric bar plus
/// the reset row — the same shape as `anthropic_api_sections` but with a
/// real percentage (Kiro always reports both used and limit) instead of an
/// optional configured one.
fn kiro_sections(s: &crate::usage::KiroSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let pct = s.pct();
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: format!("Kiro {}", s.plan),
            right: None,
        },
        Section::Spacer,
    ]);
    v.push_metric(
        Section::Metric {
            label: "Credits".into(),
            pct: pct.clamp(0, 100) as u16,
            severity: severity_for(pct),
            value_label: format!("{pct}%"),
            footnote: format!("{:.2} of {:.0}", s.used, s.limit),
        },
        s.reset_at,
    );
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Resets".into(),
        value: countdown::format(s.reset_at, now),
    });
    v
}

/// MiniMax groups quota by model bucket, so the panel is laid out by window
/// (Session, Weekly) with one row per pool — the same shape as Antigravity's
/// two-group panel. Pacing is shown: both windows report a real duration, so
/// the marker is meaningful.
fn minimax_sections(
    s: &crate::usage::MinimaxSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    use crate::minimax::vendor::{POOL_GENERAL, POOL_VIDEO};

    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    for (heading, general, video) in [
        ("Session", &s.session, s.video_session.as_ref()),
        ("Weekly", &s.weekly, s.video_weekly.as_ref()),
    ] {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: heading.into(),
            value: String::new(),
        });
        push_window(&mut v, POOL_GENERAL, general, now, tol, true);
        if let Some(w) = video {
            push_window(&mut v, POOL_VIDEO, w, now, tol, true);
        }
    }
    v
}

fn kilo_sections(s: &crate::usage::KiloSnapshot) -> SectionBuilder {
    SectionBuilder::new(vec![
        Section::Title {
            left: s.label.clone(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: usd(s.balance),
        },
    ])
}

fn novita_sections(s: &crate::usage::NovitaSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: "Novita".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: usd(s.available),
        },
        Section::Block {
            label: "Breakdown".into(),
            body: vec![format!(
                "top-up ${:.2} · credit limit ${:.2}",
                s.cash, s.credit_limit
            )],
        },
    ]);
    if s.outstanding > 0.0 {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Owed".into(),
            body: vec![usd(s.outstanding)],
        });
    }
    v
}

fn moonshot_sections(s: &crate::usage::MoonshotSnapshot) -> SectionBuilder {
    let cur = &s.currency;
    let fmt = |v: f64| money(v, cur);
    SectionBuilder::new(vec![
        Section::Title {
            left: "Kimi (Moonshot)".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: fmt(s.available),
        },
        Section::Block {
            label: "Breakdown".into(),
            body: vec![format!("cash {} · voucher {}", fmt(s.cash), fmt(s.voucher))],
        },
    ])
}

fn grok_sections(s: &crate::usage::GrokSnapshot) -> SectionBuilder {
    SectionBuilder::new(vec![
        Section::Title {
            left: "Grok (xAI)".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Prepaid balance".into(),
            value: usd(s.balance),
        },
    ])
}

fn supergrok_sections(s: &crate::usage::SuperGrokSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let pct = s.weekly_pct;
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: s.plan.clone(),
            right: None,
        },
        Section::Spacer,
    ]);
    v.push_metric(
        Section::Metric {
            label: format!("{} Build credits", s.period.label()),
            pct: pct.clamp(0, 100) as u16,
            severity: severity_for(pct),
            value_label: format!("{pct}%"),
            footnote: String::new(),
        },
        s.reset_at,
    );
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Resets".into(),
        value: countdown::format(s.reset_at, now),
    });
    if let Some(bal) = s.prepaid_balance {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "Prepaid API".into(),
            value: usd(bal),
        });
    }
    v
}

fn deepseek_sections(s: &crate::usage::DeepseekSnapshot) -> SectionBuilder {
    let currency = &s.currency;
    let fmt = |v: f64| money(v, currency);
    let avail = if s.is_available {
        "available"
    } else {
        "unavailable"
    };
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: "DeepSeek".into(),
        right: None,
    }]);
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Balance".into(),
        value: fmt(s.balance),
    });
    v.push(Section::Block {
        label: "Breakdown".into(),
        body: vec![format!(
            "granted {} · topped-up {}",
            fmt(s.granted),
            fmt(s.topped_up)
        )],
    });
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "API".into(),
        body: vec![avail.into()],
    });
    v
}

fn kimi_sections(s: &crate::usage::KimiSnapshot, now: DateTime<Utc>, _tol: u32) -> SectionBuilder {
    let plan = s.plan.as_deref().unwrap_or("Kimi");
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: plan.into(),
        right: None,
    }]);

    // Same order as every other vendor: the rolling 5h window first, the
    // weekly quota under it — the bar tiles and every panel read through a
    // session-then-weekly convention, and Kimi had it reversed.
    if s.window_limit > 0 {
        let window_pct = s.window_pct().clamp(0, 100) as u16;
        v.push(Section::Spacer);
        v.push_metric(
            Section::Metric {
                label: "Rolling window (5h)".into(),
                pct: window_pct,
                severity: severity_for(s.window_pct()),
                value_label: format!("{window_pct}%"),
                footnote: format!(
                    "{} / {} · {} remaining · reset {}",
                    s.window_used,
                    s.window_limit,
                    s.window_remaining,
                    countdown::format(s.window_reset_at, now)
                ),
            },
            s.window_reset_at,
        );
    }

    let weekly_pct = s.weekly_pct().clamp(0, 100) as u16;
    v.push(Section::Spacer);
    v.push_metric(
        Section::Metric {
            label: "Weekly quota".into(),
            pct: weekly_pct,
            severity: severity_for(s.weekly_pct()),
            // Same `{pct}%` every other vendor shows; the request counts stay
            // on the footnote.
            value_label: format!("{weekly_pct}%"),
            footnote: format!(
                "{} / {} · {} remaining · reset {}",
                s.weekly_used,
                s.weekly_limit,
                s.weekly_remaining,
                countdown::format(s.weekly_reset_at, now)
            ),
        },
        s.weekly_reset_at,
    );

    v
}

fn push_window(
    sections: &mut SectionBuilder,
    label: &str,
    w: &crate::usage::UsageWindow,
    now: DateTime<Utc>,
    tol: u32,
    show_pacing: bool,
) {
    let pct = w.utilization_pct.clamp(0, 100) as u16;
    let reset_text = countdown::format(w.resets_at, now);
    let footnote = if show_pacing {
        let p = pacing::calc(w.utilization_pct, w.resets_at, now, w.window_duration, tol);
        format!(
            "Resets in {} · {}% elapsed · {}",
            reset_text, p.elapsed_pct, p.point_label
        )
    } else {
        format!("Resets in {}", reset_text)
    };
    sections.push(Section::Spacer);
    sections.push_metric(
        Section::Metric {
            label: label.into(),
            pct,
            severity: severity_for(pct as i32),
            value_label: format!("{pct}%"),
            footnote,
        },
        w.resets_at,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_stagger_spaces_out_anthropic_tabs_only() {
        let step = Duration::from_millis(800);
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic), // default account
            TabId::account("work"),
            TabId::account("personal"),
            TabId::vendor(VendorId::Openai),
            TabId::vendor(VendorId::Zai),
        ];
        let delays = refresh_stagger(&tabs, step);
        assert_eq!(
            delays,
            vec![
                Duration::ZERO, // 1st anthropic — immediate
                step,           // 2nd anthropic
                step * 2,       // 3rd anthropic
                Duration::ZERO, // openai — own endpoint, immediate
                Duration::ZERO, // zai — own endpoint, immediate
            ]
        );
    }

    #[test]
    fn refresh_stagger_is_a_noop_without_anthropic_accounts() {
        // A single Anthropic tab (or none) never waits.
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::vendor(VendorId::Openrouter),
        ];
        assert!(
            refresh_stagger(&tabs, Duration::from_millis(800))
                .iter()
                .all(|d| d.is_zero())
        );
    }

    /// Hermetic creds for tab tests: no env vars, no CLI login.
    fn no_creds() -> DefaultCreds<'static> {
        DefaultCreds {
            env_set: &|_| false,
            kimi_cli_login: false,
        }
    }

    fn config_with_accounts(labels: &[&str]) -> Config {
        let mut config = Config::default();
        // Keep only Anthropic enabled so the test asserts on account expansion,
        // not on the full default vendor set.
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.anthropic.accounts = labels
            .iter()
            .map(|l| crate::config::AnthropicAccount {
                label: (*l).to_string(),
                credentials_path: format!("/creds/{l}.json").into(),
            })
            .collect();
        config
    }

    #[test]
    fn show_default_account_false_hides_the_unnamed_claude_tab() {
        // With named accounts and show_default_account=false, only the named
        // tabs appear — no redundant default "Claude" tab.
        let mut config = config_with_accounts(&["work", "personal"]);
        config.anthropic.show_default_account = false;
        assert_eq!(
            tabs_from_config(&config),
            vec![TabId::account("work"), TabId::account("personal")]
        );

        // But with no named accounts it is kept, so Anthropic never loses its
        // only tab.
        let mut empty = Config::default();
        empty.openai.enabled = false;
        empty.zai.enabled = false;
        empty.openrouter.enabled = false;
        empty.anthropic.show_default_account = false;
        assert_eq!(
            tabs_from_config(&empty),
            vec![TabId::vendor(VendorId::Anthropic)]
        );
    }

    #[test]
    fn tabs_expand_anthropic_accounts_after_default() {
        // Default Claude tab first, then each account in config order.
        let tabs = tabs_from_config(&config_with_accounts(&["work", "personal"]));
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("work"),
                TabId::account("personal"),
            ]
        );
    }

    #[test]
    fn tabs_without_accounts_are_just_enabled_vendors() {
        // No [[anthropic.accounts]] → one tab per enabled vendor, unchanged.
        let config = Config::default();
        let tabs = tabs_from_config(&config);
        let vendors: Vec<VendorId> = tabs.iter().map(|t| t.vendor).collect();
        assert_eq!(vendors, config.enabled_vendors());
        assert!(tabs.iter().all(|t| t.account.is_none()));
    }

    /// The reported bug: a keyless GLM/Kimi default node sits in the tab
    /// strip next to working numbered accounts. The default only keeps its
    /// tab when it carries a credential (inline key, env var, or — kimi
    /// only — the CLI's own OAuth login); `show_default_account = false`
    /// still hides even a credentialed one.
    #[test]
    fn keyless_defaults_hide_once_numbered_accounts_exist() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.openrouter.enabled = false;
        config.kimi.enabled = true;
        config.zai.accounts = vec![crate::config::ZaiAccount {
            label: "1".into(),
            api_key: Some("k1".into()),
            api_key_env: None,
            plan_tier: None,
            account_type: crate::config::ZaiAccountType::Personal,
            organization_id: None,
            project_id: None,
            site: None,
        }];
        config.kimi.accounts = vec![crate::config::KimiAccount {
            label: "1".into(),
            api_key: Some("k2".into()),
            api_key_env: None,
            monthly_limit: None,
        }];
        let tabs = build_tabs(&config, &[], &no_creds());
        assert_eq!(
            tabs,
            vec![
                TabId::account_for(VendorId::Zai, "1"),
                TabId::account_for(VendorId::Kimi, "1"),
            ],
            "keyless defaults are dead nodes — gone"
        );

        // A credentialed kimi default (the CLI login) keeps its tab.
        let creds_with_login = DefaultCreds {
            env_set: &|_| false,
            kimi_cli_login: true,
        };
        let tabs = build_tabs(&config, &[], &creds_with_login);
        assert_eq!(
            tabs,
            vec![
                TabId::account_for(VendorId::Zai, "1"),
                TabId::vendor(VendorId::Kimi),
                TabId::account_for(VendorId::Kimi, "1"),
            ]
        );

        // An env-var default counts as credentialed too.
        let creds_with_env = DefaultCreds {
            env_set: &|name| name == "ZAI_API_KEY",
            kimi_cli_login: false,
        };
        let tabs = build_tabs(&config, &[], &creds_with_env);
        assert_eq!(tabs[0], TabId::vendor(VendorId::Zai));

        // show_default_account = false hides even a credentialed default.
        config.kimi.api_key = Some("default-kimi".into());
        config.kimi.show_default_account = false;
        let tabs = build_tabs(&config, &[], &no_creds());
        assert_eq!(
            tabs,
            vec![
                TabId::account_for(VendorId::Zai, "1"),
                TabId::account_for(VendorId::Kimi, "1"),
            ]
        );

        // No accounts at all: the default shows even unconfigured (the
        // fresh-install entry point).
        config.zai.accounts.clear();
        config.kimi.accounts.clear();
        config.kimi.show_default_account = true;
        config.kimi.api_key = None;
        let tabs = build_tabs(&config, &[], &no_creds());
        assert_eq!(
            tabs,
            vec![TabId::vendor(VendorId::Zai), TabId::vendor(VendorId::Kimi)]
        );
    }

    #[test]
    fn tabs_expand_openrouter_accounts_without_changing_other_vendors() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.accounts = vec![
            crate::config::OpenRouterAccount {
                label: "work".into(),
                api_key_env: Some("OPENROUTER_WORK_API_KEY".into()),
                api_key: None,

                monthly_limit: None,
            },
            crate::config::OpenRouterAccount {
                label: "personal".into(),
                api_key_env: None,
                api_key: Some("personal-key".into()),

                monthly_limit: None,
            },
        ];
        // The default keeps its tab because it carries its own inline key —
        // a CREDENTIALED default next to numbered accounts stays.
        config.openrouter.api_key = Some("default-key".into());
        assert_eq!(
            build_tabs(&config, &[], &no_creds()),
            vec![
                TabId::vendor(VendorId::Openrouter),
                TabId::account_for(VendorId::Openrouter, "work"),
                TabId::account_for(VendorId::Openrouter, "personal"),
            ]
        );
        // A keyless default is a dead node once accounts exist — hidden.
        config.openrouter.api_key = None;
        assert_eq!(
            build_tabs(&config, &[], &no_creds()),
            vec![
                TabId::account_for(VendorId::Openrouter, "work"),
                TabId::account_for(VendorId::Openrouter, "personal"),
            ]
        );
    }

    #[test]
    fn openrouter_can_hide_default_only_when_named_accounts_exist() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.show_default_account = false;
        assert_eq!(
            build_tabs(&config, &[], &no_creds()),
            vec![TabId::vendor(VendorId::Openrouter)]
        );

        config
            .openrouter
            .accounts
            .push(crate::config::OpenRouterAccount {
                label: "work".into(),
                api_key_env: Some("OPENROUTER_WORK_API_KEY".into()),
                api_key: None,

                monthly_limit: None,
            });
        assert_eq!(
            build_tabs(&config, &[], &no_creds()),
            vec![TabId::account_for(VendorId::Openrouter, "work")]
        );
    }

    #[test]
    fn tabs_include_accounts_auto_discovered_from_accounts_dir() {
        // A CLAUDE_CONFIG_DIR-style directory becomes account tabs with no
        // explicit [[anthropic.accounts]] entry. Hermetic: real TempDir.
        let td = tempfile::tempdir().unwrap();
        for label in ["work", "personal"] {
            let dir = td.path().join(label);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
        }
        let mut config = Config::default();
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.anthropic.accounts_dir = Some(td.path().to_path_buf());

        let tabs = tabs_from_config(&config);
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("personal"), // sorted by label
                TabId::account("work"),
            ]
        );
    }

    #[test]
    fn desktop_labels_become_account_tabs_after_cli_accounts() {
        // Pure core: desktop accounts follow CLI accounts, in the order given.
        let config = config_with_accounts(&["work"]);
        let tabs = build_tabs(&config, &["gmail".into(), "hotmail".into()], &no_creds());
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("work"),
                TabId::desktop_account("gmail"),
                TabId::desktop_account("hotmail"),
            ]
        );
    }

    #[test]
    fn a_desktop_profile_wins_a_label_collision_with_a_cli_account() {
        // One tab per label; the Desktop source wins so the label is never fed
        // from two stores refreshing one rotating token (which invalidate each
        // other and can silently show a wrong account's usage). The CLI entry is
        // dropped; a CLI-only label (work) is untouched.
        let config = config_with_accounts(&["gmail", "work"]);
        let tabs = build_tabs(&config, &["gmail".into(), "hotmail".into()], &no_creds());
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("work"),
                TabId::desktop_account("gmail"),
                TabId::desktop_account("hotmail"),
            ]
        );
    }

    #[test]
    fn desktop_accounts_suppress_the_default_tab_like_named_ones() {
        // show_default_account=false + only Desktop accounts => no default tab,
        // exactly as if they were [[anthropic.accounts]] (the Desktop-only user).
        let mut config = config_with_accounts(&[]);
        config.cursor.enabled = false;
        config.anthropic.show_default_account = false;

        // No accounts of either kind: the default tab survives (never leave
        // Anthropic tab-less).
        assert_eq!(
            build_tabs(&config, &[], &no_creds()),
            vec![TabId::vendor(VendorId::Anthropic)]
        );
        // A Desktop account is present: default suppressed, only the account.
        assert_eq!(
            build_tabs(&config, &["gmail".into()], &no_creds()),
            vec![TabId::desktop_account("gmail")]
        );
    }
}
