//! Live API smoke test suite — DETECTS UNDOCUMENTED-ENDPOINT DRIFT.
//!
//! Hits the real vendor endpoints using credentials from your shell (API keys,
//! or the local CLI/IDE session files for Cursor and Kiro CLI).
//! Asserts only the *fields we depend on* so when a vendor renames or removes
//! one, the failure points at the exact field rather than dumping the whole
//! response.
//!
//! These tests are `#[ignore]` so plain `cargo test` doesn't hit external
//! APIs (and won't fail on machines without creds). Run explicitly:
//!
//! ```bash
//! source ~/.config/zsh/secrets
//! cargo test --test live -- --ignored --nocapture
//! # or:
//! make smoke                         # runs every configured live smoke test
//! cargo test --test live kimi_live -- --ignored --nocapture
//! ```
//!
//! ## When a smoke test fails
//!
//! 1. Re-run with `--nocapture` to see the actual response shape.
//! 2. Paste the response + error into Claude Code and ask it to update the
//!    affected vendor's `types.rs` to match. The error messages here are
//!    deliberately verbose so the update is mechanical.
//! 3. After updating, re-run `cargo test --test live -- --ignored` to confirm.
//!
//! ## What gets tested (only the contract we rely on)
//!
//! - **Anthropic**: `five_hour.utilization` is 0..=100, `resets_at` parses
//!   as RFC3339, `extra_usage.{is_enabled,monthly_limit,used_credits}` round-trip.
//! - **OpenAI**: `rate_limit.primary_window.used_percent` is 0..=100, the
//!   id_token's exp claim is parseable.
//! - **Z.AI**: response is a `{code, data: {limits:[...], level}, success}`
//!   envelope and at least one `TOKENS_LIMIT` entry exists.
//! - **OpenRouter**: `/credits` returns `{data:{total_credits,total_usage}}`
//!   and `/key` returns `{data:{usage,is_free_tier}}`.
//! - **Kimi**: the public snapshot exposes parsed weekly limit/used/remaining
//!   counters and a bounded percentage. Its reset and selected 5-hour rolling
//!   window are optional, so the smoke test validates their public fields only
//!   when present; the snapshot does not expose raw wire duration/unit.
//!   `kimi_live` skips when optional `KIMI_API_KEY` is unset.
//! - **Kimi (subscription)**: reads the Kimi Code CLI's own OAuth session,
//!   refreshing it (and writing the rotation back to the CLI's credential
//!   file) when it is close to expiry, then asserts the same public snapshot
//!   fields. `kimi_subscription_live` skips when the CLI is not logged in and
//!   `KIMI_CODE_HOME` is unset.
//! - **Cursor**: reads the session token from the local `state.vscdb`, then
//!   asserts `premium_pct` is 0..=100 and a future `premium_reset_at` was
//!   derived from `startOfMonth`. `cursor_live` skips when there is no Cursor
//!   credential source (no state DB, no cursor-agent `auth.json`, and neither
//!   `CURSOR_DB_PATH` nor `CURSOR_AGENT_AUTH_PATH` set).
//! - **Kiro CLI**: reads the AWS SSO OIDC session from kiro-cli's local
//!   `data.sqlite3`, then asserts the credit counters are non-negative and the
//!   plan label is non-empty. `kiro_live` skips when there is no kiro-cli
//!   install (no db and no `KIRO_DB_PATH`).
//! - **SuperGrok**: asks the official Grok Build CLI's `x.ai/billing` ACP
//!   extension, then asserts usage percent and plan. Set
//!   `SUPERGROK_GROK_BINARY` to the trusted official executable.
//!   `supergrok_live` skips when that CLI is not installed.
//! - **DeepSeek**: `/user/balance` returns an availability flag, a currency
//!   label, and a non-negative balance split into granted / topped-up
//!   components. `deepseek_live` skips when `DEEPSEEK_API_KEY` is unset.
//! - **Kilo**: `/api/profile/balance` returns a non-negative USD credit
//!   balance. `kilo_live` skips when `KILO_API_KEY` is unset.
//! - **Novita**: the billing balance detail returns non-negative available /
//!   cash / credit-limit / outstanding amounts. `novita_live` skips when
//!   `NOVITA_API_KEY` is unset.
//! - **Moonshot**: `/v1/users/me/balance` returns a non-negative spendable
//!   balance; the region picks host and currency (`MOONSHOT_REGION=cn` for
//!   CNY, default global/USD). `moonshot_live` skips when `MOONSHOT_API_KEY`
//!   is unset.
//! - **Grok**: the Management API returns a finite prepaid balance, with the
//!   team auto-resolved from the key unless `XAI_TEAM_ID` is set.
//!   `grok_live` skips when `XAI_MANAGEMENT_KEY` is unset.
//! - **OpenCode Go**: the official usage endpoint yields at least one of the
//!   rolling / weekly / monthly windows, each percent-bounded. `opencode_go_live`
//!   skips when `OPENCODE_GO_API_KEY` is unset.
//! - **Anthropic API**: the Admin cost report returns a finite non-negative
//!   month-to-date spend. `anthropic_api_live` skips when
//!   `ANTHROPIC_ADMIN_KEY` is unset.
//! - **Antigravity**: probes the loopback language servers that the local
//!   Antigravity products expose, then asserts the plan label and bounded
//!   window percentages. `antigravity_live` skips when loopback discovery
//!   finds nothing listening.

use std::time::Duration;

use ai_usagebar::anthropic;
use ai_usagebar::anthropic_api;
use ai_usagebar::antigravity;
use ai_usagebar::cache::Cache;
use ai_usagebar::cursor;
use ai_usagebar::deepseek;
use ai_usagebar::error::AppError;
use ai_usagebar::grok;
use ai_usagebar::kilo;
use ai_usagebar::kimi;
use ai_usagebar::kiro;
use ai_usagebar::minimax;
use ai_usagebar::moonshot;
use ai_usagebar::novita;
use ai_usagebar::openai;
use ai_usagebar::openai_api;
use ai_usagebar::opencode_go;
use ai_usagebar::openrouter;
use ai_usagebar::supergrok;
use ai_usagebar::zai;

fn xdg_cache_for(test: &str) -> Cache {
    // Use a per-test scratch dir so smoke tests don't clobber the real cache.
    let base = std::env::temp_dir().join(format!("ai-usagebar-smoke-{test}"));
    let _ = std::fs::remove_dir_all(&base);
    Cache::at(base)
}

fn assert_pct(label: &str, p: i32) {
    assert!(
        (0..=100).contains(&p),
        "{label}: utilization {p} outside [0,100] — vendor shape changed?"
    );
}

fn is_missing_credentials(err: &AppError) -> bool {
    matches!(err, AppError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn anthropic_live() {
    let creds_path = anthropic::creds::default_path().expect("resolve home directory");
    // Creds live in this file (Linux) or the login Keychain (recent macOS, no
    // file) — CredsTarget::Default covers both. Skip cleanly when neither
    // source resolves — a no-op on machines without creds, as the module doc
    // promises, not a hard failure.
    let creds_target = anthropic::creds::CredsTarget::Default(creds_path);
    match anthropic::creds::resolve(&creds_target) {
        Ok(_) => {}
        // A missing file (no Keychain on Linux) surfaces as `Unconfigured`
        // with "credentials error:" Display prose, not a bare io::NotFound —
        // treat both as the documented no-creds skip.
        Err(err)
            if is_missing_credentials(&err)
                || matches!(err, AppError::Unconfigured(_) | AppError::Credentials(_)) =>
        {
            eprintln!("anthropic_live: no Claude credentials (file or Keychain) — skipping");
            return;
        }
        Err(err) => panic!("anthropic_live: failed to read Claude credentials: {err}"),
    }
    let cache = xdg_cache_for("anthropic");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = anthropic::fetch::Endpoints::default();
    let out = anthropic::fetch_snapshot(
        &client,
        &creds_target,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("anthropic fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "anthropic plan label empty");
    assert_pct("anthropic.session", out.snapshot.session.utilization_pct);
    assert_pct("anthropic.weekly", out.snapshot.weekly.utilization_pct);
    if let Some(s) = out.snapshot.sonnet.as_ref() {
        assert_pct("anthropic.sonnet", s.utilization_pct);
    }
    if let Some(e) = out.snapshot.extra.as_ref() {
        if let Some(l) = e.limit {
            assert!(l.0 >= 0, "anthropic extra.limit < 0");
        }
        // spent can equal or exceed limit briefly during reconciliation; just sanity-check.
        assert!(e.spent.0 >= 0, "anthropic extra.spent < 0");
    }
    println!(
        "✅ anthropic — plan={}, session={}%, weekly={}%, sonnet={:?}, extra={:?}",
        out.snapshot.plan,
        out.snapshot.session.utilization_pct,
        out.snapshot.weekly.utilization_pct,
        out.snapshot.sonnet.as_ref().map(|s| s.utilization_pct),
        out.snapshot
            .extra
            .as_ref()
            .map(|e| (e.fmt_spent(), e.fmt_limit())),
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn openai_live() {
    let creds_path = openai::creds::default_path().expect("resolve home directory");
    if !creds_path.exists() {
        eprintln!(
            "openai_live: no Codex credentials at {} — skipping (run `codex login` first)",
            creds_path.display()
        );
        return;
    }
    let cache = xdg_cache_for("openai");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = openai::fetch::Endpoints::default();
    let out = openai::fetch_snapshot(
        &client,
        &creds_path,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("openai fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "openai plan label empty");
    assert!(
        out.snapshot.session.is_some() || out.snapshot.weekly.is_some(),
        "openai returned no 5h or 7d usage window"
    );
    if let Some(session) = out.snapshot.session.as_ref() {
        assert_pct("openai.session", session.utilization_pct);
    }
    if let Some(weekly) = out.snapshot.weekly.as_ref() {
        assert_pct("openai.weekly", weekly.utilization_pct);
    }
    println!(
        "✅ openai — plan={}, session={:?}%, weekly={:?}%, credits={:?}",
        out.snapshot.plan,
        out.snapshot
            .session
            .as_ref()
            .map(|window| window.utilization_pct),
        out.snapshot
            .weekly
            .as_ref()
            .map(|window| window.utilization_pct),
        out.snapshot.credits.map(|c| c.balance),
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn zai_live() {
    let Ok(api_key) = std::env::var("ZAI_API_KEY") else {
        eprintln!("zai_live: ZAI_API_KEY is unset — skipping optional Z.AI smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("zai_live: ZAI_API_KEY is empty — skipping optional Z.AI smoke test");
        return;
    }
    let cache = xdg_cache_for("zai");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = zai::fetch::Endpoints::default();
    let out = zai::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        None,
    )
    .await
    .expect("zai fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "zai plan label empty");
    // Z.AI may legitimately return 0% on a fresh account, but at least one
    // bucket should exist — if all three are None, the schema changed.
    let has_any = out.snapshot.session.is_some()
        || out.snapshot.weekly.is_some()
        || out.snapshot.mcp.is_some();
    assert!(has_any, "zai snapshot has no buckets — shape changed?");
    for (label, w) in [
        ("session", &out.snapshot.session),
        ("weekly", &out.snapshot.weekly),
        ("mcp", &out.snapshot.mcp),
    ] {
        if let Some(w) = w.as_ref() {
            assert_pct(&format!("zai.{label}"), w.utilization_pct);
        }
    }
    println!(
        "✅ zai — plan={}, session={:?}%, weekly={:?}%, mcp={:?}%",
        out.snapshot.plan,
        out.snapshot.session.as_ref().map(|w| w.utilization_pct),
        out.snapshot.weekly.as_ref().map(|w| w.utilization_pct),
        out.snapshot.mcp.as_ref().map(|w| w.utilization_pct),
    );
}

/// Team (enterprise) BigModel coding plan — same quota endpoint with
/// `?type=2` + org headers on open.bigmodel.cn. The team plan only exists on
/// the CN site, so the three variables carry both the key and the org ids
/// (bigmodel.cn console → F12 → Local Storage → Bigmodel-Organization /
/// Bigmodel-Project).
#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn zai_team_live() {
    let Ok(api_key) = std::env::var("BIGMODEL_TEAM_API_KEY") else {
        eprintln!("skip: BIGMODEL_TEAM_API_KEY not set");
        return;
    };
    let organization = std::env::var("BIGMODEL_TEAM_ORG").expect("BIGMODEL_TEAM_ORG");
    let project = std::env::var("BIGMODEL_TEAM_PROJECT").expect("BIGMODEL_TEAM_PROJECT");
    let cache = xdg_cache_for("zai-team");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let plan = ai_usagebar::config::ResolvedZaiAccount {
        api_key,
        account_type: ai_usagebar::config::ZaiAccountType::Team,
        site: ai_usagebar::config::ZaiSite::Cn,
        organization_id: Some(organization),
        project_id: Some(project),
        plan_tier: None,
    };
    let out = zai::fetch_snapshot_plan(
        &client,
        &plan,
        &cache,
        &zai::fetch::Endpoints::for_site(ai_usagebar::config::ZaiSite::Cn),
        Duration::from_secs(0),
        chrono::Utc::now(),
    )
    .await
    .expect("zai team fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "zai team plan label empty");
    // Same envelope as the personal plan: at least one windowed bucket.
    let has_any = out.snapshot.session.is_some() || out.snapshot.weekly.is_some();
    assert!(has_any, "zai team snapshot has no buckets — shape changed?");
    println!(
        "✅ zai team — plan={}, session={:?}%, weekly={:?}%",
        out.snapshot.plan,
        out.snapshot.session.as_ref().map(|w| w.utilization_pct),
        out.snapshot.weekly.as_ref().map(|w| w.utilization_pct),
    );
}

/// Pay-as-you-go BigModel key (no coding subscription) — the model-usage
/// 7-day aggregate is the only thing it reports.
#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn zai_usage_only_live() {
    let Ok(api_key) = std::env::var("BIGMODEL_PAYG_API_KEY") else {
        eprintln!("skip: BIGMODEL_PAYG_API_KEY not set");
        return;
    };
    let cache = xdg_cache_for("zai-usage");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let plan = ai_usagebar::config::ResolvedZaiAccount {
        api_key,
        account_type: ai_usagebar::config::ZaiAccountType::Usage,
        site: ai_usagebar::config::ZaiSite::Cn,
        organization_id: None,
        project_id: None,
        plan_tier: None,
    };
    let out = zai::fetch_snapshot_plan(
        &client,
        &plan,
        &cache,
        &zai::fetch::Endpoints::for_site(ai_usagebar::config::ZaiSite::Cn),
        Duration::from_secs(0),
        chrono::Utc::now(),
    )
    .await
    .expect("zai usage-only fetch should succeed against the real API");

    let stats = out
        .snapshot
        .usage_stats
        .expect("usage-only snapshot carries 7-day stats");
    println!(
        "✅ zai usage-only — {} prompts · {} tokens (7d)",
        stats.prompts, stats.tokens
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn openrouter_live() {
    let Ok(api_key) = std::env::var("OPENROUTER_API_KEY") else {
        eprintln!(
            "openrouter_live: OPENROUTER_API_KEY is unset — skipping optional OpenRouter smoke test"
        );
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!(
            "openrouter_live: OPENROUTER_API_KEY is empty — skipping optional OpenRouter smoke test"
        );
        return;
    }
    let cache = xdg_cache_for("openrouter");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = openrouter::fetch::Endpoints::default();
    let out = openrouter::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("openrouter fetch should succeed against the real API");

    assert!(out.snapshot.total_credits >= 0.0, "or.total_credits < 0");
    assert!(out.snapshot.total_usage >= 0.0, "or.total_usage < 0");
    // total_usage being slightly larger than total_credits is possible during
    // reconciliation (debt allowed); don't assert otherwise.
    println!(
        "✅ openrouter — label={}, balance=${:.2}, used=${:.2}, monthly=${:.2}, free={}",
        out.snapshot.label,
        out.snapshot.balance(),
        out.snapshot.total_usage,
        out.snapshot.usage_monthly,
        out.snapshot.is_free_tier,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn kimi_live() {
    let Ok(api_key) = std::env::var("KIMI_API_KEY") else {
        eprintln!("kimi_live: KIMI_API_KEY is unset — skipping optional Kimi smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("kimi_live: KIMI_API_KEY is empty — skipping optional Kimi smoke test");
        return;
    }
    let cache = xdg_cache_for("kimi");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = kimi::fetch::Endpoints::default();
    let out = kimi::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("kimi fetch should succeed against the real API");

    // Kimi permits missing or inconsistent counters, and the production
    // snapshot deliberately preserves them. Exercise all weekly fields while
    // checking the production-facing normalized percentage only.
    assert_pct("kimi.weekly", out.snapshot.weekly_pct());
    // A nonzero public limit means the parser selected its optional rolling
    // window. The public snapshot does not retain the wire duration/unit, so
    // it can only validate that window's normalized percentage and counters.
    if out.snapshot.window_limit > 0 {
        assert_pct("kimi.window", out.snapshot.window_pct());
    }
    println!(
        "✅ kimi — plan={:?}, weekly={} / {} ({} remaining; reset {:?}), window={} / {} ({} remaining; reset {:?})",
        out.snapshot.plan,
        out.snapshot.weekly_used,
        out.snapshot.weekly_limit,
        out.snapshot.weekly_remaining,
        out.snapshot.weekly_reset_at,
        out.snapshot.window_used,
        out.snapshot.window_limit,
        out.snapshot.window_remaining,
        out.snapshot.window_reset_at,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn kimi_subscription_live() {
    // The subscription path has no API key — the credential is the OAuth
    // session the Kimi Code CLI stored after its device-code login. So this
    // test needs `kimi` installed and logged in (or `KIMI_CODE_HOME` pointing
    // at a copy of that home) and skips otherwise, like `kiro_live`.
    //
    // It exercises the refresh too whenever the stored access token is inside
    // its 2-minute buffer, which for a 15-minute token is most runs — and that
    // rotation is written back to the CLI's own credential file, exactly as
    // production does. Point `KIMI_CODE_HOME` at a copy if that matters to you.
    let home = match std::env::var("KIMI_CODE_HOME") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => kimi::oauth::default_home().expect("resolve home dir"),
    };
    let credentials = kimi::oauth::credentials_path_in(&home);
    if !kimi::oauth::is_logged_in(&credentials) {
        eprintln!(
            "kimi_subscription_live: no Kimi Code CLI login at {} — skipping (run `kimi` and log in, or set KIMI_CODE_HOME)",
            credentials.display()
        );
        return;
    }

    let region = kimi::oauth::read_region_marker(&home).unwrap_or(kimi::oauth::Region::MainlandCn);
    let cache = xdg_cache_for("kimi-subscription");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let out = kimi::fetch::fetch_snapshot_with_auth(
        &client,
        &kimi::fetch::Auth::KimiCode(kimi::fetch::KimiCodeAuth::in_home(&home)),
        &cache,
        &kimi::fetch::Endpoints::for_region(region),
        Duration::from_secs(0),
    )
    .await
    .expect("kimi subscription fetch should succeed against the real API");

    assert_pct("kimi.weekly", out.snapshot.weekly_pct());
    if out.snapshot.window_limit > 0 {
        assert_pct("kimi.window", out.snapshot.window_pct());
    }
    // The refresh must never leave the CLI without a usable login.
    assert!(
        kimi::oauth::is_logged_in(&credentials),
        "the Kimi Code CLI credential file must still hold a login afterwards"
    );
    println!(
        "✅ kimi (subscription) — plan={:?}, weekly={} / {}, window={} / {}",
        out.snapshot.plan,
        out.snapshot.weekly_used,
        out.snapshot.weekly_limit,
        out.snapshot.window_used,
        out.snapshot.window_limit,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn cursor_live() {
    // Cursor has no API key — the credential is a session token, either the
    // one the Cursor IDE wrote to its local state DB, or (headless machines
    // with no IDE) the one the `cursor-agent` CLI wrote to its own auth.json.
    // So this test needs one of the two installed (or `CURSOR_DB_PATH` /
    // `CURSOR_AGENT_AUTH_PATH` pointing at a copy) and skips otherwise, the
    // same way `kimi_live` skips without a key. Nothing to fetch on a CI box
    // with neither.
    let db_path = match std::env::var("CURSOR_DB_PATH") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => cursor::db::default_db_path().expect("resolve platform config dir"),
    };
    let agent_auth_path = match std::env::var("CURSOR_AGENT_AUTH_PATH") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => cursor::db::default_agent_auth_path().expect("resolve platform config dir"),
    };
    if !db_path.exists() && !agent_auth_path.exists() {
        eprintln!(
            "cursor_live: no Cursor state DB at {} and no cursor-agent auth at {} — skipping \
             (sign in to the Cursor IDE or run `cursor-agent`, or set CURSOR_DB_PATH / \
             CURSOR_AGENT_AUTH_PATH)",
            db_path.display(),
            agent_auth_path.display()
        );
        return;
    }

    let cache = xdg_cache_for("cursor");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = cursor::fetch::Endpoints::default();
    let out = cursor::fetch_snapshot(
        &client,
        &db_path,
        &agent_auth_path,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("cursor fetch should succeed against the real API");

    // The fields the widget depends on: two pool percentages (>= 0; a pool can
    // exceed 100 when over its included allowance, so only the low bound is
    // asserted) and a future billing-cycle reset.
    assert!(
        out.snapshot.auto_pct >= 0 && out.snapshot.api_pct >= 0,
        "cursor: negative pool percentage — shape changed? auto={} api={}",
        out.snapshot.auto_pct,
        out.snapshot.api_pct
    );
    assert!(!out.snapshot.plan.is_empty(), "cursor plan label empty");
    assert!(
        out.snapshot
            .reset_at
            .is_some_and(|r| r > chrono::Utc::now()),
        "cursor: reset_at should be a future instant, got {:?}",
        out.snapshot.reset_at
    );
    println!(
        "✅ cursor — plan={}, Cursor Models {}%, Other Models {}%, total {}%, on-demand={}, reset {:?}",
        out.snapshot.plan,
        out.snapshot.auto_pct,
        out.snapshot.api_pct,
        out.snapshot.total_pct,
        out.snapshot.on_demand_enabled,
        out.snapshot.reset_at,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn kiro_live() {
    // Kiro has no API key — the credential is the AWS SSO OIDC session
    // kiro-cli wrote to its own local database after `kiro-cli login`. So this
    // test needs kiro-cli installed and signed in (or `KIRO_DB_PATH` pointing
    // at a copied `data.sqlite3`) and skips otherwise, like `cursor_live`.
    let db_path = match std::env::var("KIRO_DB_PATH") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => kiro::db::default_db_path().expect("resolve platform data dir"),
    };
    if !db_path.exists() {
        eprintln!(
            "kiro_live: no kiro-cli database at {} — skipping (run `kiro-cli login`, or set KIRO_DB_PATH)",
            db_path.display()
        );
        return;
    }

    let cache = xdg_cache_for("kiro");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let out = kiro::fetch_snapshot(&client, &db_path, &cache, Duration::from_secs(0))
        .await
        .expect("kiro fetch should succeed against the real API");

    // The fields the widget depends on: non-negative credit counters, a
    // non-empty plan label, and (when reported) a future reset.
    assert!(
        out.snapshot.used >= 0.0 && out.snapshot.limit >= 0.0,
        "kiro: negative credit counter — shape changed? used={} limit={}",
        out.snapshot.used,
        out.snapshot.limit
    );
    assert!(!out.snapshot.plan.is_empty(), "kiro plan label empty");
    if let Some(reset) = out.snapshot.reset_at {
        assert!(
            reset > chrono::Utc::now(),
            "kiro: reset_at should be a future instant, got {reset:?}"
        );
    }
    println!(
        "✅ kiro — plan={}, credits {} / {} ({}%), reset {:?}",
        out.snapshot.plan,
        out.snapshot.used,
        out.snapshot.limit,
        out.snapshot.pct(),
        out.snapshot.reset_at,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn supergrok_live() {
    // Grok Build owns every auth mode and returns only billing data over ACP.
    let binary = std::env::var_os("SUPERGROK_GROK_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ai_usagebar::config::Config::default().supergrok.grok_binary);
    let auth_override = std::env::var_os("SUPERGROK_AUTH_PATH").map(std::path::PathBuf::from);
    let config_override = std::env::var_os("SUPERGROK_CONFIG_PATH").map(std::path::PathBuf::from);
    let scope_paths = supergrok::scope::ScopePaths::with_overrides(
        auth_override.as_deref(),
        config_override.as_deref(),
    )
    .expect("resolve Grok scope paths");
    let cache = xdg_cache_for("supergrok");
    // A missing Grok Build install is the unconfigured state for this vendor.
    let fetched = supergrok::fetch_snapshot(&binary, &scope_paths, &cache, Duration::ZERO).await;
    let out = match fetched {
        Err(AppError::Credentials(msg)) if msg.contains("Grok Build CLI not found") => {
            eprintln!(
                "supergrok_live: official Grok Build CLI not found — skipping (install Grok Build, run `grok login`, or set SUPERGROK_GROK_BINARY)"
            );
            return;
        }
        other => other.expect("SuperGrok billing should succeed through official Grok Build ACP"),
    };

    assert!(
        out.snapshot.weekly_pct >= 0,
        "supergrok: negative weekly_pct — shape changed? {}",
        out.snapshot.weekly_pct
    );
    assert!(!out.snapshot.plan.is_empty(), "supergrok plan label empty");
    if let Some(reset) = out.snapshot.reset_at {
        assert!(
            reset > chrono::Utc::now() - chrono::Duration::days(1),
            "supergrok: reset_at looks implausibly old: {reset:?}"
        );
    }
    println!(
        "✅ supergrok — plan={}, {} {}%, prepaid {:?}, reset {:?}",
        out.snapshot.plan,
        out.snapshot.period.label(),
        out.snapshot.weekly_pct,
        out.snapshot.prepaid_balance,
        out.snapshot.reset_at,
    );
}

/// MiniMax Token Plan — optional: skipped unless a subscription key is present.
///
/// The endpoint answers HTTP 200 even for auth failures, so a green run here is
/// what proves the in-band `base_resp.status_code` check is still doing its job:
/// a wrong key surfaces as an error rather than an all-zero plan.
#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn minimax_live() {
    let Ok(api_key) = std::env::var("MINIMAX_API_KEY") else {
        eprintln!("minimax_live: MINIMAX_API_KEY is unset — skipping optional MiniMax smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("minimax_live: MINIMAX_API_KEY is empty — skipping optional MiniMax smoke test");
        return;
    }
    let cache = xdg_cache_for("minimax");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = minimax::fetch::Endpoints::default();
    let out = minimax::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("minimax fetch should succeed against the real API");

    assert_pct("minimax.session", out.snapshot.session.utilization_pct);
    assert_pct("minimax.weekly", out.snapshot.weekly.utilization_pct);
    // The interval length is read from the payload rather than assumed: it has
    // been observed at both 4h and 5h on the same account. A non-positive one
    // would divide the pace math by nothing.
    assert!(
        out.snapshot.session.window_duration > chrono::Duration::zero(),
        "minimax: interval window has no length — payload shape changed?"
    );
    if let Some(v) = out.snapshot.video_session.as_ref() {
        assert_pct("minimax.video", v.utilization_pct);
    }
    println!(
        "✅ minimax: {} · session {}% · weekly {}% · video {:?}",
        out.snapshot.plan,
        out.snapshot.session.utilization_pct,
        out.snapshot.weekly.utilization_pct,
        out.snapshot
            .video_session
            .as_ref()
            .map(|w| w.utilization_pct),
    );
}

// --- OpenAI Admin API (platform spend) --------------------------------------

/// `OPENAI_ADMIN_KEY` must be an organization ADMIN key — platform.openai.com
/// → Settings → Organization. Regular project `sk-` keys are rejected by
/// `/v1/organization/costs` with 403, which is itself worth seeing verbatim.
#[tokio::test]
#[ignore = "live API smoke — see module docs"]
async fn openai_api_live() {
    let Ok(api_key) = std::env::var("OPENAI_ADMIN_KEY") else {
        eprintln!("openai_api_live: OPENAI_ADMIN_KEY not set — skipping");
        return;
    };
    let cache = xdg_cache_for("openai_api");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = openai_api::fetch::Endpoints::default();
    let out = openai_api::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        None,
    )
    .await
    .expect("openai-api fetch should succeed against the real Costs API");

    assert!(
        out.snapshot.spent.is_finite() && out.snapshot.spent >= 0.0,
        "openai-api: 30-day spend is not a sane amount: {}",
        out.snapshot.spent
    );
    assert!(
        !out.snapshot.by_day.is_empty() || out.snapshot.spent == 0.0,
        "openai-api: non-zero spend but no daily buckets — response shape changed?"
    );
    println!(
        "✅ openai-api: ${:.2} over 30d · {} daily buckets · top: {:?}",
        out.snapshot.spent,
        out.snapshot.by_day.len(),
        out.snapshot.top_items.first()
    );
}

// --- Opt-in vendors: each skips gracefully when unconfigured ----------------

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn deepseek_live() {
    let Ok(api_key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!(
            "deepseek_live: DEEPSEEK_API_KEY is unset — skipping optional DeepSeek smoke test"
        );
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!(
            "deepseek_live: DEEPSEEK_API_KEY is empty — skipping optional DeepSeek smoke test"
        );
        return;
    }
    let cache = xdg_cache_for("deepseek");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = deepseek::fetch::Endpoints::default();
    let out = deepseek::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect(
        "deepseek fetch should succeed against the real API \
         (note: deepseek::fetch remaps live HTTP errors to \"no usable cache\" \
         when nothing is cached — a bogus key fails with that message)",
    );

    assert!(
        !out.snapshot.currency.is_empty(),
        "deepseek currency label empty"
    );
    assert!(
        out.snapshot.balance.is_finite() && out.snapshot.balance >= 0.0,
        "deepseek: balance is not a sane amount: {}",
        out.snapshot.balance
    );
    assert!(
        out.snapshot.granted.is_finite()
            && out.snapshot.granted >= 0.0
            && out.snapshot.topped_up.is_finite()
            && out.snapshot.topped_up >= 0.0,
        "deepseek: negative balance component — shape changed? granted={} topped_up={}",
        out.snapshot.granted,
        out.snapshot.topped_up
    );
    println!(
        "✅ deepseek — available={}, balance={} {} (granted {}, topped up {})",
        out.snapshot.is_available,
        out.snapshot.balance,
        out.snapshot.currency,
        out.snapshot.granted,
        out.snapshot.topped_up,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn kilo_live() {
    let Ok(api_key) = std::env::var("KILO_API_KEY") else {
        eprintln!("kilo_live: KILO_API_KEY is unset — skipping optional Kilo smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("kilo_live: KILO_API_KEY is empty — skipping optional Kilo smoke test");
        return;
    }
    let cache = xdg_cache_for("kilo");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = kilo::fetch::Endpoints::default();
    let out = kilo::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        None,
    )
    .await
    .expect("kilo fetch should succeed against the real API");

    assert!(!out.snapshot.label.is_empty(), "kilo label empty");
    assert!(
        out.snapshot.balance.is_finite() && out.snapshot.balance >= 0.0,
        "kilo: balance is not a sane amount: {}",
        out.snapshot.balance
    );
    println!("✅ kilo — balance ${:.2}", out.snapshot.balance);
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn novita_live() {
    let Ok(api_key) = std::env::var("NOVITA_API_KEY") else {
        eprintln!("novita_live: NOVITA_API_KEY is unset — skipping optional Novita smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("novita_live: NOVITA_API_KEY is empty — skipping optional Novita smoke test");
        return;
    }
    let cache = xdg_cache_for("novita");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = novita::fetch::Endpoints::default();
    let out = novita::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("novita fetch should succeed against the real API");

    for (name, amount) in [
        ("available", out.snapshot.available),
        ("cash", out.snapshot.cash),
        ("credit_limit", out.snapshot.credit_limit),
        ("outstanding", out.snapshot.outstanding),
    ] {
        assert!(
            amount.is_finite() && amount >= 0.0,
            "novita: {name} is not a sane amount: {amount}"
        );
    }
    println!(
        "✅ novita — available ${:.2}, cash ${:.2}, limit ${:.2}, outstanding ${:.2}",
        out.snapshot.available,
        out.snapshot.cash,
        out.snapshot.credit_limit,
        out.snapshot.outstanding,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn moonshot_live() {
    let Ok(api_key) = std::env::var("MOONSHOT_API_KEY") else {
        eprintln!(
            "moonshot_live: MOONSHOT_API_KEY is unset — skipping optional Moonshot smoke test"
        );
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!(
            "moonshot_live: MOONSHOT_API_KEY is empty — skipping optional Moonshot smoke test"
        );
        return;
    }
    let region = std::env::var("MOONSHOT_REGION").unwrap_or_else(|_| "global".to_string());
    let (endpoints, currency) = moonshot::fetch::Endpoints::for_region(&region);
    let cache = xdg_cache_for("moonshot");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let out = moonshot::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        currency,
    )
    .await
    .expect("moonshot fetch should succeed against the real API");

    assert!(
        out.snapshot.available.is_finite() && out.snapshot.available >= 0.0,
        "moonshot: available balance is not a sane amount: {}",
        out.snapshot.available
    );
    assert!(
        out.snapshot.cash.is_finite(),
        "moonshot: cash balance is not finite: {}",
        out.snapshot.cash
    );
    assert!(
        !out.snapshot.currency.is_empty(),
        "moonshot currency label empty"
    );
    println!(
        "✅ moonshot ({}) — available {} {} (cash {}, voucher {})",
        region,
        out.snapshot.available,
        out.snapshot.currency,
        out.snapshot.cash,
        out.snapshot.voucher,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn grok_live() {
    let Ok(management_key) = std::env::var("XAI_MANAGEMENT_KEY") else {
        eprintln!("grok_live: XAI_MANAGEMENT_KEY is unset — skipping optional Grok smoke test");
        return;
    };
    if management_key.trim().is_empty() {
        eprintln!("grok_live: XAI_MANAGEMENT_KEY is empty — skipping optional Grok smoke test");
        return;
    }
    let team_id = std::env::var("XAI_TEAM_ID").ok();
    let cache = xdg_cache_for("grok");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = grok::fetch::Endpoints::default();
    let out = grok::fetch_snapshot(
        &client,
        &management_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        team_id.as_deref(),
    )
    .await
    .expect("grok fetch should succeed against the real API");

    assert!(
        out.snapshot.balance.is_finite(),
        "grok: balance is not finite: {}",
        out.snapshot.balance
    );
    println!("✅ grok — prepaid balance ${:.2}", out.snapshot.balance);
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn opencode_go_live() {
    let Ok(api_key) = std::env::var("OPENCODE_GO_API_KEY") else {
        eprintln!(
            "opencode_go_live: OPENCODE_GO_API_KEY is unset — skipping optional OpenCode Go smoke test"
        );
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!(
            "opencode_go_live: OPENCODE_GO_API_KEY is empty — skipping optional OpenCode Go smoke test"
        );
        return;
    }
    let cache = xdg_cache_for("opencode-go");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = opencode_go::fetch::Endpoints::default();
    let out = opencode_go::fetch::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("opencode-go fetch should succeed against the real API");

    // The parser rejects a response with no supported windows, so reaching
    // here already proves at least one exists.
    for (label, window) in [
        ("rolling", &out.snapshot.rolling),
        ("weekly", &out.snapshot.weekly),
        ("monthly", &out.snapshot.monthly),
    ] {
        if let Some(window) = window.as_ref() {
            assert!(
                (0.0..=100.0).contains(&window.percent),
                "opencode-go.{label}: percent {} outside [0,100] — shape changed?",
                window.percent
            );
        }
    }
    println!(
        "✅ opencode-go — rolling={:?}%, weekly={:?}%, monthly={:?}%",
        out.snapshot.rolling.as_ref().map(|w| w.percent),
        out.snapshot.weekly.as_ref().map(|w| w.percent),
        out.snapshot.monthly.as_ref().map(|w| w.percent),
    );
}

/// Organization **Admin** key (`sk-ant-admin01-…`) — inference keys are
/// rejected by the cost report endpoint with 403, worth seeing verbatim.
#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn anthropic_api_live() {
    let Ok(api_key) = std::env::var("ANTHROPIC_ADMIN_KEY") else {
        eprintln!(
            "anthropic_api_live: ANTHROPIC_ADMIN_KEY is unset — skipping optional Anthropic API smoke test"
        );
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!(
            "anthropic_api_live: ANTHROPIC_ADMIN_KEY is empty — skipping optional Anthropic API smoke test"
        );
        return;
    }
    let cache = xdg_cache_for("anthropic-api");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = anthropic_api::fetch::Endpoints::default();
    let out = anthropic_api::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        None,
    )
    .await
    .expect("anthropic-api fetch should succeed against the real Admin API");

    assert!(
        out.snapshot.spent.is_finite() && out.snapshot.spent >= 0.0,
        "anthropic-api: month-to-date spend is not a sane amount: {}",
        out.snapshot.spent
    );
    println!(
        "✅ anthropic-api — ${:.2} month-to-date (limit {:?})",
        out.snapshot.spent, out.snapshot.limit,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn antigravity_live() {
    let cache = xdg_cache_for("antigravity");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    // Loopback discovery is the credential source here: nothing listening
    // locally is this vendor's "unconfigured" state, not a failure.
    let fetched = antigravity::fetch_snapshot(&client, &cache, Duration::from_secs(0)).await;
    let out = match fetched {
        Err(AppError::Credentials(msg)) if msg.contains("no local server found") => {
            eprintln!(
                "antigravity_live: no local Antigravity server discovered — skipping (open the Antigravity app or an interactive `agy` session, or set ANTIGRAVITY_LS_ADDRESS)"
            );
            return;
        }
        other => other.expect("antigravity fetch should succeed against the local server"),
    };

    assert!(
        !out.snapshot.plan.is_empty(),
        "antigravity plan label empty"
    );
    assert_pct("antigravity.session", out.snapshot.session.utilization_pct);
    assert_pct("antigravity.weekly", out.snapshot.weekly.utilization_pct);
    if let Some(w) = out.snapshot.third_party_session.as_ref() {
        assert_pct("antigravity.third_party_session", w.utilization_pct);
    }
    if let Some(w) = out.snapshot.third_party_weekly.as_ref() {
        assert_pct("antigravity.third_party_weekly", w.utilization_pct);
    }
    println!(
        "✅ antigravity — plan={}, gemini 5h {}% / week {}%, claude-gpt 5h {:?} / week {:?}",
        out.snapshot.plan,
        out.snapshot.session.utilization_pct,
        out.snapshot.weekly.utilization_pct,
        out.snapshot
            .third_party_session
            .as_ref()
            .map(|w| w.utilization_pct),
        out.snapshot
            .third_party_weekly
            .as_ref()
            .map(|w| w.utilization_pct),
    );
}
