//! Shared cached-fetch pipeline — the uniform scaffold every key-based
//! vendor repeats around its live HTTP call: ensure the cache dir, take the
//! flock, serve a fresh payload without polling the network, and on failure
//! fall back to the last good payload with the three-arm error policy.
//!
//! Migration contract (todos 11-14): the scaffold is shared, the policy is
//! vendor-specific (`src/AGENTS.md` convention "vendor-specific logic stays
//! in the vendor module"):
//! - Cache formats are private per vendor, so each vendor passes its own
//!   `parse_cache` / `to_json` closures; this module never parses snapshots.
//! - When no cache is usable, the ORIGINAL error is returned UNMAPPED.
//!   Vendors whose fallback text differs (deepseek's synthetic Transport
//!   message vs novita's original error — both deliberate, pinned by tests
//!   and why-comments in the vendor files) rewrite it in their own wrapper
//!   around [`run_cached_fetch`].
//! - `lock_timeout` and `cache_ttl` are VALUEs, not policy hooks: callers
//!   pass `Duration::from_secs(LOCK_TIMEOUT_SECS)` or
//!   `Duration::from_secs(LOCK_TIMEOUT_OAUTH_SECS)` and forward their
//!   existing per-call cache TTL (60s default, 300s anthropic/openai gentle
//!   throttle), mirroring every existing `fetch_snapshot`.
//! - Vendor-specific pre/post logic (zai's pre-validation + degrade chain,
//!   OAuth token refresh, identity checks) stays inside the caller's
//!   live-fetch future; antigravity (loopback RPC) and claude_desktop are
//!   deliberate non-migrants.
//!
//! Cache API note: `cache::acquire_lock_async` already takes its wait as a
//! `Duration` parameter, so no cache.rs change was needed — each vendor's
//! effective lock-wait behavior is preserved by the constants below.

use std::future::Future;
use std::time::Duration;

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
use crate::error::{AppError, Result};

/// HTTP timeout for the standard REST vendors (10s across all 14 today).
pub const HTTP_TIMEOUT_DEFAULT_SECS: u64 = 10;

/// HTTP timeout for the admin-API vendors (`anthropic_api`, `openai_api`),
/// whose slower endpoints keep their own 15s.
pub const HTTP_TIMEOUT_ADMIN_API_SECS: u64 = 15;

/// Default flock wait for the cached-fetch pipeline (15s).
pub const LOCK_TIMEOUT_SECS: u64 = 15;

/// Flock wait for the OAuth vendors (`anthropic`, `openai`): the lock is
/// held across their ~25s token refresh, so the default 15s would abandon a
/// sibling multi-monitor poller mid-refresh (verified in each vendor's
/// fetch.rs).
pub const LOCK_TIMEOUT_OAUTH_SECS: u64 = 45;

/// Generic outcome of a cached fetch. Field set and order copy
/// `deepseek/fetch.rs` verbatim so a vendor migration is the type alias
/// `pub type FetchOutcome = crate::fetch::FetchOutcome<XSnapshot>;` —
/// `cache_age` is `std::time::Duration` like every vendor copy today (the
/// plan text says `chrono::Duration`, but that would break the drop-in
/// alias and add a dependency the vendors never had).
#[derive(Debug, Clone)]
pub struct FetchOutcome<S> {
    pub snapshot: S,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// Run the shared lock / TTL / fallback scaffold around a vendor's live
/// fetch.
///
/// Order of operations (identical to every vendor pipeline today):
/// `ensure_dir` → `acquire_lock_async(cache.lock_path(), lock_timeout)` →
/// `fresh_payload(cache_ttl)` reuse (non-stale) when parseable → otherwise
/// await `live_fetch`; on `Ok` write the payload and return a fresh
/// outcome, on `Err` apply the three-arm failure policy — transient
/// ([`AppError::is_transient`]): no `write_last_error`, no `mark_stale`;
/// `AppError::Http { status, body }`: `write_last_error(status, &body)`;
/// anything else: `write_last_error(0, &to_string)` — then reuse the stale
/// cache when parseable (`stale = true`, `last_error` set per the arm
/// above), else return the error UNMAPPED.
pub async fn run_cached_fetch<S>(
    cache: &Cache,
    lock_timeout: Duration,
    cache_ttl: Duration,
    live_fetch: impl Future<Output = Result<S>>,
    parse_cache: impl Fn(&str) -> Option<S>,
    to_json: impl Fn(&S) -> String,
) -> Result<FetchOutcome<S>> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), lock_timeout).await?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Some(snapshot) = parse_cached(&bytes, &parse_cache)
    {
        return Ok(FetchOutcome {
            snapshot,
            stale: false,
            last_error: cache.read_last_error(),
            cache_age: cache.payload_age(),
        });
    }
    // A fresh-but-corrupt cache falls through to the live fetch rather than
    // surfacing fabricated data (same contract as the vendor pipelines).

    match live_fetch.await {
        Ok(snapshot) => {
            let json = to_json(&snapshot);
            cache.write_payload(json.as_bytes())?;
            Ok(FetchOutcome {
                snapshot,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback(cache, cache.read_last_error(), e, &parse_cache),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(status, &body));
            fallback(
                cache,
                last_error,
                AppError::Http { status, body },
                &parse_cache,
            )
        }
        Err(e) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(0, &e.to_string()));
            fallback(cache, last_error, e, &parse_cache)
        }
    }
}

/// Decode a cached payload with the vendor's parser. Non-UTF-8 counts as
/// unparseable: the caller falls through to the live fetch.
fn parse_cached<S>(bytes: &[u8], parse_cache: &impl Fn(&str) -> Option<S>) -> Option<S> {
    let text = std::str::from_utf8(bytes).ok()?;
    parse_cache(text)
}

/// Stale-cache fallback shared by every failure arm: reuse the last good
/// payload (within `MAX_STALE`) with `last_error` attached, or surface
/// `original` UNMAPPED when nothing usable is cached — with nothing to
/// show, hiding what went wrong helps nobody (novita's why-comment; vendors
/// with their own fallback text rewrite it in their wrapper).
fn fallback<S>(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    original: AppError,
    parse_cache: &impl Fn(&str) -> Option<S>,
) -> Result<FetchOutcome<S>> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    let Some(snapshot) = parse_cached(&bytes, parse_cache) else {
        return Err(original);
    };
    Ok(FetchOutcome {
        snapshot,
        stale: true,
        last_error,
        cache_age: cache.payload_age(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestSnap {
        value: u32,
    }

    fn parse(s: &str) -> Option<TestSnap> {
        serde_json::from_str(s).ok()
    }

    fn to_json(s: &TestSnap) -> String {
        serde_json::to_string(s).unwrap()
    }

    fn fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("shared"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn lock_timeout() -> Duration {
        Duration::from_secs(LOCK_TIMEOUT_SECS)
    }

    #[tokio::test]
    async fn fresh_cache_hit_returns_non_stale_snapshot_without_polling_live() {
        let (_td, cache) = fixture();
        cache.write_payload(br#"{"value":7}"#).unwrap();
        let polled = AtomicBool::new(false);
        let live = async {
            polled.store(true, Ordering::SeqCst);
            Err::<TestSnap, AppError>(AppError::Other("must not run".into()))
        };

        let out = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(60),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap();

        assert_eq!(out.snapshot, TestSnap { value: 7 });
        assert!(!out.stale);
        assert_eq!(out.last_error, None);
        assert!(
            !polled.load(Ordering::SeqCst),
            "a fresh cache hit must never poll the live future"
        );
    }

    #[tokio::test]
    async fn ok_live_fetch_writes_the_cache_and_returns_a_fresh_outcome() {
        let (_td, cache) = fixture();
        let live = async { Ok::<TestSnap, AppError>(TestSnap { value: 3 }) };

        let out = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(60),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap();

        assert!(!out.stale);
        assert_eq!(out.snapshot, TestSnap { value: 3 });
        assert_eq!(out.last_error, None);
        assert_eq!(out.cache_age, Some(Duration::ZERO));
        // The payload landed on disk and parses back to the same snapshot.
        let bytes = std::fs::read(cache.payload_path()).unwrap();
        assert_eq!(
            parse(std::str::from_utf8(&bytes).unwrap()),
            Some(TestSnap { value: 3 })
        );
    }

    #[tokio::test]
    async fn http_error_with_warm_cache_returns_stale_outcome_carrying_last_error() {
        let (_td, cache) = fixture();
        cache.write_payload(br#"{"value":9}"#).unwrap();
        let live = async {
            Err::<TestSnap, AppError>(AppError::Http {
                status: 500,
                body: "provider exploded".into(),
            })
        };

        // TTL 0 deterministically forces the live path: `age < 0` is never
        // true, so the just-written payload is never "fresh".
        let out = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(0),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot, TestSnap { value: 9 });
        assert_eq!(out.last_error, Some((500, "provider exploded".into())));
        // The persisted marker matches the pair the outcome carries.
        assert_eq!(
            cache.read_last_error(),
            Some((500, "provider exploded".into()))
        );
    }

    #[tokio::test]
    async fn http_error_with_empty_cache_returns_the_original_error_unmapped() {
        let (_td, cache) = fixture();
        let live = async {
            Err::<TestSnap, AppError>(AppError::Http {
                status: 404,
                body: "gone".into(),
            })
        };

        let err = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(60),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap_err();

        // UNMAPPED: the error is byte-identical to the live one; the shared
        // core invents no synthetic "no usable cache" text — that policy is
        // vendor-side (deepseek rewrites it, novita does not).
        assert_eq!(
            err.to_string(),
            AppError::Http {
                status: 404,
                body: "gone".into()
            }
            .to_string()
        );
    }

    #[tokio::test]
    async fn transient_error_with_empty_cache_returns_the_error_and_writes_no_markers() {
        let (_td, cache) = fixture();
        let live =
            async { Err::<TestSnap, AppError>(AppError::Transport("network unreachable".into())) };

        let err = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(60),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap_err();

        // No silent fake data: nothing cached means the failure surfaces.
        assert_eq!(
            err.to_string(),
            "network transport error: network unreachable"
        );
        assert!(!cache.last_error_path().exists());
        assert!(!cache.stale_path().exists());
    }

    /// Pins the fallback_silent arm, which no vendor test covered: a
    /// transient failure must not poison the cache with error markers.
    #[tokio::test]
    async fn transient_error_with_warm_cache_serves_stale_cache_without_error_markers() {
        let (_td, cache) = fixture();
        cache.write_payload(br#"{"value":11}"#).unwrap();
        let live = async { Err::<TestSnap, AppError>(AppError::Transport("timeout".into())) };

        let out = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(0),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot, TestSnap { value: 11 });
        assert_eq!(out.last_error, None, "transient failures stay silent");
        assert!(!cache.is_stale());
        assert!(!cache.last_error_path().exists());
    }

    #[tokio::test]
    async fn non_http_error_arm_persists_code_zero_with_the_error_text() {
        let (_td, cache) = fixture();
        cache.write_payload(br#"{"value":2}"#).unwrap();
        let live = async { Err::<TestSnap, AppError>(AppError::Schema("shape drift".into())) };

        let out = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(0),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(
            out.last_error,
            Some((0, AppError::Schema("shape drift".into()).to_string()))
        );
    }

    #[tokio::test]
    async fn corrupt_fresh_cache_falls_through_to_the_live_fetch() {
        let (_td, cache) = fixture();
        cache.write_payload(b"not json at all").unwrap();
        let live = async { Ok::<TestSnap, AppError>(TestSnap { value: 5 }) };

        let out = run_cached_fetch(
            &cache,
            lock_timeout(),
            Duration::from_secs(60),
            live,
            parse,
            to_json,
        )
        .await
        .unwrap();

        assert!(!out.stale);
        assert_eq!(out.snapshot, TestSnap { value: 5 });
        // The corrupt payload was replaced by the live one.
        assert_eq!(
            cache.maybe_payload().unwrap(),
            Some(br#"{"value":5}"#.to_vec())
        );
    }
}
