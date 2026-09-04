//! End-to-end integration tests for the zai, openrouter, and openai vendors.
//!
//! Distinct from the in-module mockito unit suites (which pin one fetch arm
//! or one render in isolation): these drive the WHOLE pipeline a Waybar
//! module runs — mockito server (loopback only) → `fetch_snapshot` with an
//! injected `Endpoints` and `Cache::at(TempDir)` → the vendor's `render` →
//! `WaybarOutput` — and pin the bar text and tooltip via insta snapshots.
//! `now` is pinned so countdowns and pace glyphs are deterministic; the
//! "Updated HH:MM" tooltip footer is normalized away because it renders
//! local time derived from the real cache mtime.
//!
//! Bodies are copied from each vendor's types.rs doc examples and existing
//! unit fixtures (reset timestamps chosen relative to the pinned now:
//! 2026-05-23T12:00:00Z).

// allow: SIZE_OK — the plan mandates ONE integration file for three vendors;
// the bulk is fixture bodies + snapshot plumbing, not branching logic.
use std::io::Write;
use std::time::Duration;

use ai_usagebar::cache::Cache;
use ai_usagebar::error::AUTH_FAILURE_MESSAGE;
use ai_usagebar::format::updated_at_hm;
use ai_usagebar::theme::Theme;
use ai_usagebar::vendor::{RenderOpts, VendorOutcome};
use ai_usagebar::{AppError, openai, openrouter, zai};
use chrono::{DateTime, TimeZone, Utc};
use tempfile::{NamedTempFile, TempDir};

/// Pinned clock — every countdown, pace glyph, and snapshot derives from it.
fn pinned_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap()
}

fn cache_in(td: &TempDir, vendor: &str) -> Cache {
    let c = Cache::at(td.path().join(vendor));
    c.ensure_dir().unwrap();
    c
}

fn opts() -> RenderOpts {
    RenderOpts {
        format: None,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
    }
}

/// The "Updated HH:MM" tooltip footer is local time computed from the real
/// cache mtime — replace it so snapshots stay deterministic across
/// timezones and runs (same normalization as tests/anthropic_e2e.rs).
fn normalize_updated_time(tooltip: &str, cache_age: Option<Duration>) -> String {
    let expected = format!("Updated {}", updated_at_hm(pinned_now(), cache_age));
    assert!(
        tooltip.contains(&expected),
        "tooltip did not contain {expected:?}"
    );
    tooltip.replace(&expected, "Updated HH:MM")
}

// ---------------------------------------------------------------- zai ----

/// Quota envelope from zai/types.rs' doc example with live figures: 5h at
/// 42% (+2h05m), weekly at 15% (+3d4h30m), monthly MCP ceiling at 30%.
const ZAI_QUOTA_FULL: &str = r#"{"code":200,"msg":"Operation successful","data":{
    "limits":[
        {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42,"nextResetTime":1779545100000},
        {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":15,"nextResetTime":1779813000000},
        {"type":"TIME_LIMIT","unit":5,"number":1,"usage":1000,"currentValue":300,"remaining":700,
         "percentage":30,"nextResetTime":1780574400000,
         "usageDetails":[{"modelCode":"search-prime","usage":0}]}
    ],
    "level":"pro"
},"success":true}"#;

fn zai_endpoints(server: &mockito::Server) -> zai::fetch::Endpoints {
    let url = server.url();
    zai::fetch::Endpoints {
        quota: format!("{url}/api/monitor/usage/quota/limit"),
        usage_stats: format!("{url}/api/monitor/usage/model-usage"),
    }
}

async fn zai_fetch(
    server: &mockito::Server,
    cache: &Cache,
) -> Result<zai::fetch::FetchOutcome, AppError> {
    zai::fetch::fetch_snapshot(
        &reqwest::Client::new(),
        "fake-key",
        cache,
        &zai_endpoints(server),
        Duration::from_secs(0),
        None,
    )
    .await
}

#[tokio::test]
async fn zai_full_response_renders_expected_waybar_json() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/monitor/usage/quota/limit")
        .with_status(200)
        .with_body(ZAI_QUOTA_FULL)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "zai");
    let outcome = zai_fetch(&server, &cache).await.unwrap();
    assert!(!outcome.stale);
    assert_eq!(outcome.snapshot.plan, "GLM Coding Pro");

    let vo = VendorOutcome::from(outcome.clone());
    let out = zai::vendor::render(
        &vo,
        &outcome.snapshot,
        &Theme::default(),
        &opts(),
        pinned_now(),
    );
    assert_eq!(format!("{:?}", out.class), "Low"); // worst window 42% → green
    insta::assert_snapshot!("zai_full_response_bar_text", &out.text);
    insta::assert_snapshot!(
        "zai_full_response_tooltip",
        normalize_updated_time(&out.tooltip, outcome.cache_age)
    );
}

/// With nothing cached, the API's own error surfaces — the old synthetic
/// "no usable cache" masked the real message (pinned in-module; here it is
/// the e2e shape: fetch-level error, no WaybarOutput).
#[tokio::test]
async fn zai_http_401_without_cache_surfaces_the_api_error() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/monitor/usage/quota/limit")
        .with_status(401)
        .with_body(r#"{"code":401,"msg":"Unauthorized key"}"#)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "zai");
    let err = zai_fetch(&server, &cache).await.unwrap_err();
    assert!(
        matches!(err, AppError::Http { status: 401, .. }),
        "expected the raw 401, got {err:?}"
    );
    assert!(err.to_string().contains("Unauthorized key"), "{err}");
    assert!(cache.maybe_payload().unwrap().is_none());
}

#[tokio::test]
async fn zai_http_500_falls_back_to_stale_cache_rendering_the_error() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/monitor/usage/quota/limit")
        .with_status(500)
        .with_body(r#"{"code":500,"msg":"upstream exploded"}"#)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "zai");
    cache.write_payload(ZAI_QUOTA_FULL.as_bytes()).unwrap();

    let outcome = zai_fetch(&server, &cache).await.unwrap();
    assert!(outcome.stale);
    assert_eq!(outcome.snapshot.plan, "GLM Coding Pro");
    assert_eq!(outcome.last_error.as_ref().map(|(c, _)| *c), Some(500));

    let vo = VendorOutcome::from(outcome.clone());
    let out = zai::vendor::render(
        &vo,
        &outcome.snapshot,
        &Theme::default(),
        &opts(),
        pinned_now(),
    );
    assert!(out.text.contains('⏸'));
    assert!(out.tooltip.contains("HTTP 500"));
    insta::assert_snapshot!(
        "zai_http_500_stale_tooltip",
        normalize_updated_time(&out.tooltip, outcome.cache_age)
    );
}

/// A 200 whose body has a wrong-typed field is schema drift, never usage —
/// and a validated-payload-only cache means it is not written either.
#[tokio::test]
async fn zai_malformed_body_is_schema_drift_not_usage() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/monitor/usage/quota/limit")
        .with_status(200)
        .with_body(
            r#"{"code":200,"msg":"Operation successful","data":{"limits":[
                {"type":"TOKENS_LIMIT","unit":3,"percentage":"forty-two"}
            ]},"success":true}"#,
        )
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "zai");
    let err = zai_fetch(&server, &cache).await.unwrap_err();
    assert!(err.to_string().contains("zai quota response"), "{err}");
    assert!(cache.maybe_payload().unwrap().is_none());
}

// --------------------------------------------------------- openrouter ----

/// Both endpoint bodies copied from openrouter/fetch.rs' in-module mockito
/// fixtures (label "prod", $100 credits / $25.50 spent, per-key limit $50).
const OR_CREDITS_FULL: &str = r#"{"data":{"total_credits":100.0,"total_usage":25.5}}"#;
const OR_KEY_FULL: &str = r#"{"data":{"label":"prod","limit":50.0,"limit_remaining":24.5,
    "usage":25.5,"usage_daily":1.0,"usage_weekly":7.0,"usage_monthly":25.5,
    "is_free_tier":false}}"#;

fn or_endpoints(server: &mockito::Server) -> openrouter::fetch::Endpoints {
    let url = server.url();
    openrouter::fetch::Endpoints {
        credits: format!("{url}/api/v1/credits"),
        key: format!("{url}/api/v1/key"),
    }
}

async fn or_fetch(
    server: &mockito::Server,
    cache: &Cache,
) -> Result<openrouter::fetch::FetchOutcome, AppError> {
    openrouter::fetch::fetch_snapshot(
        &reqwest::Client::new(),
        "sk-or-test",
        cache,
        &or_endpoints(server),
        Duration::from_secs(0),
    )
    .await
}

/// The cache-repr seed openrouter writes (`{"snapshot": …}`), shaped like the
/// in-module stale-fallback fixture.
fn or_seed_cache(cache: &Cache) {
    let seed = serde_json::json!({
        "snapshot": {
            "label": "OpenRouter — seed", "total_credits": 50.0,
            "total_usage": 10.0, "usage_daily": 1.0, "usage_weekly": 3.0,
            "usage_monthly": 10.0, "is_free_tier": false,
            "limit": null, "limit_remaining": null
        }
    });
    cache.write_payload(seed.to_string().as_bytes()).unwrap();
}

#[tokio::test]
async fn openrouter_full_response_renders_expected_waybar_json() {
    let mut server = mockito::Server::new_async().await;
    for (path, body) in [
        ("/api/v1/credits", OR_CREDITS_FULL),
        ("/api/v1/key", OR_KEY_FULL),
    ] {
        server
            .mock("GET", path)
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
    }

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openrouter");
    let outcome = or_fetch(&server, &cache).await.unwrap();
    assert!(!outcome.stale);
    assert_eq!(outcome.snapshot.label, "OpenRouter — prod");

    let vo = VendorOutcome::from(outcome.clone());
    let out = openrouter::vendor::render(
        &vo,
        &outcome.snapshot,
        &Theme::default(),
        &opts(),
        pinned_now(),
    );
    assert_eq!(format!("{:?}", out.class), "Low"); // 26% consumed → green
    insta::assert_snapshot!("openrouter_full_response_bar_text", &out.text);
    insta::assert_snapshot!(
        "openrouter_full_response_tooltip",
        normalize_updated_time(&out.tooltip, outcome.cache_age)
    );
}

/// OpenRouter's fallback policy is deepseek-shaped: with nothing cached, a
/// live failure becomes the stable synthetic text instead of the raw error.
#[tokio::test]
async fn openrouter_http_401_without_cache_reports_the_synthetic_error() {
    let mut server = mockito::Server::new_async().await;
    for path in ["/api/v1/credits", "/api/v1/key"] {
        server
            .mock("GET", path)
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;
    }

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openrouter");
    let err = or_fetch(&server, &cache).await.unwrap_err();
    assert!(
        matches!(err, AppError::Other(ref t) if t == "openrouter: no usable cache"),
        "synthetic no-usable-cache text was rewritten: {err:?}"
    );
}

#[tokio::test]
async fn openrouter_http_500_falls_back_to_stale_cache_rendering_the_error() {
    let mut server = mockito::Server::new_async().await;
    for path in ["/api/v1/credits", "/api/v1/key"] {
        server
            .mock("GET", path)
            .with_status(500)
            .with_body(r#"{"error":"exploded"}"#)
            .create_async()
            .await;
    }

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openrouter");
    or_seed_cache(&cache);

    let outcome = or_fetch(&server, &cache).await.unwrap();
    assert!(outcome.stale);
    assert_eq!(outcome.snapshot.label, "OpenRouter — seed");
    assert_eq!(outcome.last_error.as_ref().map(|(c, _)| *c), Some(500));

    let vo = VendorOutcome::from(outcome.clone());
    let out = openrouter::vendor::render(
        &vo,
        &outcome.snapshot,
        &Theme::default(),
        &opts(),
        pinned_now(),
    );
    assert!(out.text.contains('⏸'));
    assert!(out.tooltip.contains("HTTP 500"));
    insta::assert_snapshot!(
        "openrouter_http_500_stale_tooltip",
        normalize_updated_time(&out.tooltip, outcome.cache_age)
    );
}

#[tokio::test]
async fn openrouter_malformed_body_is_schema_drift_and_never_cached() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/credits")
        .with_status(200)
        .with_body(r#"{"data":{"total_credits":"lots","total_usage":0.0}}"#)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/key")
        .with_status(200)
        .with_body(OR_KEY_FULL)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openrouter");
    let err = or_fetch(&server, &cache).await.unwrap_err();
    // Same synthetic text as any no-cache failure; the recorded pair's code 0
    // is what distinguishes the schema arm from an HTTP failure (401/500).
    assert!(
        matches!(err, AppError::Other(ref t) if t == "openrouter: no usable cache"),
        "{err:?}"
    );
    assert_eq!(
        cache.read_last_error().map(|(code, _)| code),
        Some(0),
        "the schema arm persists code 0"
    );
    assert!(cache.maybe_payload().unwrap().is_none());
}

// -------------------------------------------------------------- openai ----

/// Usage body from openai/types.rs' captured real shape: 5h at 37% (+2h),
/// weekly at 80% (+5d3h), code review at 22% (+6d22h), credits block.
const OAI_USAGE_FULL: &str = r#"{
    "user_id":"u","account_id":"a","email":"e@example.com",
    "plan_type":"plus",
    "rate_limit":{"allowed":true,"limit_reached":false,
        "primary_window":{"used_percent":37,"limit_window_seconds":18000,"reset_at":1779544800},
        "secondary_window":{"used_percent":80,"limit_window_seconds":604800,"reset_at":1779980400}
    },
    "code_review_rate_limit":{
        "primary_window":{"used_percent":22,"limit_window_seconds":604800,"reset_at":1780135200}
    },
    "credits":{"balance":"$2.50","has_credits":true,"unlimited":false,
        "approx_local_messages":[100,200],"approx_cloud_messages":[30,50]}
}"#;

/// Raw upstream body used as the stale-cache seed (openai caches the raw
/// UsageResponse bytes, not a snapshot projection).
const OAI_USAGE_STALE_SEED: &str = r#"{"plan_type":"pro","rate_limit":{
    "primary_window":{"used_percent":50,"limit_window_seconds":18000,"reset_at":1779544800}}}"#;

/// Unsigned fake JWT carrying the claims fetch reads (exp + plan type).
fn fake_jwt(claims: serde_json::Value) -> String {
    use base64::Engine;
    let h =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
    format!("{h}.{p}.sig")
}

/// Fresh Codex creds in a TempDir (same fixture shape as openai/fetch.rs'
/// in-module tests): exp 1h out, so no token refresh fires during the test.
fn openai_creds() -> NamedTempFile {
    let exp = Utc::now().timestamp() + 3600;
    let jwt = fake_jwt(serde_json::json!({
        "exp": exp,
        "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"}
    }));
    let mut f = NamedTempFile::new().unwrap();
    write!(
        f,
        r#"{{"tokens":{{"access_token":"AT","refresh_token":"RT","id_token":"{jwt}","account_id":"acc"}}}}"#
    )
    .unwrap();
    f.flush().unwrap();
    f
}

async fn oai_fetch(
    server: &mockito::Server,
    creds_path: &std::path::Path,
    cache: &Cache,
) -> Result<openai::fetch::FetchOutcome, AppError> {
    openai::fetch::fetch_snapshot(
        &reqwest::Client::new(),
        creds_path,
        cache,
        &openai::fetch::Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/v1/oauth/token", server.url()),
        },
        Duration::from_secs(0),
    )
    .await
}

#[tokio::test]
async fn openai_full_response_renders_expected_waybar_json() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/backend-api/wham/usage")
        .with_status(200)
        .with_body(OAI_USAGE_FULL)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openai");
    let creds = openai_creds();
    let outcome = oai_fetch(&server, creds.path(), &cache).await.unwrap();
    assert!(!outcome.stale);
    assert_eq!(outcome.snapshot.plan, "ChatGPT Plus");

    let vo = VendorOutcome::from(outcome.clone());
    let out = openai::vendor::render(
        &vo,
        &outcome.snapshot,
        &Theme::default(),
        &opts(),
        pinned_now(),
    );
    assert_eq!(format!("{:?}", out.class), "High"); // weekly 80% is the worst
    insta::assert_snapshot!("openai_full_response_bar_text", &out.text);
    insta::assert_snapshot!(
        "openai_full_response_tooltip",
        normalize_updated_time(&out.tooltip, outcome.cache_age)
    );
}

#[tokio::test]
async fn openai_http_401_without_cache_fails_and_redacts_the_persisted_body() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/backend-api/wham/usage")
        .with_status(401)
        .with_body(r#"{"error":{"message":"bad token for user@example.com"}}"#)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openai");
    let creds = openai_creds();
    let err = oai_fetch(&server, creds.path(), &cache).await.unwrap_err();
    assert!(
        matches!(err, AppError::Other(ref t) if t == "openai: no usable cache"),
        "{err:?}"
    );
    // 401 bodies never reach disk verbatim — cache::write_last_error redacts.
    assert_eq!(
        cache.read_last_error(),
        Some((401, AUTH_FAILURE_MESSAGE.to_string()))
    );
}

#[tokio::test]
async fn openai_http_500_falls_back_to_stale_cache_rendering_the_error() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/backend-api/wham/usage")
        .with_status(500)
        .with_body(r#"{"error":{"message":"upstream exploded"}}"#)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openai");
    cache
        .write_payload(OAI_USAGE_STALE_SEED.as_bytes())
        .unwrap();

    let creds = openai_creds();
    let outcome = oai_fetch(&server, creds.path(), &cache).await.unwrap();
    assert!(outcome.stale);
    assert_eq!(outcome.snapshot.plan, "ChatGPT Pro");
    assert_eq!(outcome.last_error.as_ref().map(|(c, _)| *c), Some(500));

    let vo = VendorOutcome::from(outcome.clone());
    let out = openai::vendor::render(
        &vo,
        &outcome.snapshot,
        &Theme::default(),
        &opts(),
        pinned_now(),
    );
    assert!(out.text.contains('⏸'));
    assert!(out.tooltip.contains("HTTP 500"));
    insta::assert_snapshot!(
        "openai_http_500_stale_tooltip",
        normalize_updated_time(&out.tooltip, outcome.cache_age)
    );
}

#[tokio::test]
async fn openai_malformed_body_is_schema_drift_and_never_cached() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/backend-api/wham/usage")
        .with_status(200)
        .with_body(
            r#"{"plan_type":"plus","rate_limit":{"primary_window":{
                "used_percent":"n/a","limit_window_seconds":18000}}}"#,
        )
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td, "openai");
    let creds = openai_creds();
    let err = oai_fetch(&server, creds.path(), &cache).await.unwrap_err();
    assert!(
        matches!(err, AppError::Other(ref t) if t == "openai: no usable cache"),
        "{err:?}"
    );
    assert_eq!(
        cache.read_last_error().map(|(code, _)| code),
        Some(0),
        "the schema arm persists code 0"
    );
    assert!(cache.maybe_payload().unwrap().is_none());
}
