//! Novita AI fetch — reads the account balance from
//! `/openapi/v1/billing/balance/detail` under the shared cache + flock
//! primitives. Single-endpoint balance, same shape as `kilo::fetch`.

use std::time::Duration;

use crate::cache::Cache;
use crate::error::{AppError, Result};
use crate::fetch::{HTTP_TIMEOUT_DEFAULT_SECS, LOCK_TIMEOUT_SECS, run_cached_fetch};
use crate::usage::{NovitaSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{BalanceData, to_snapshot};

pub const BASE_URL: &str = "https://api.novita.ai";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub balance: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            balance: format!("{BASE_URL}/openapi/v1/billing/balance/detail"),
        }
    }
}

pub type FetchOutcome = crate::fetch::FetchOutcome<NovitaSnapshot>;

/// On failure we show the last good figure with the error alongside it. With
/// nothing usable cached there is nothing to show, so the **original** error is
/// returned rather than a generic "no usable cache" that hides what went wrong
/// — exactly the fallback policy the shared `run_cached_fetch` implements, so
/// this wrapper returns its result unmapped.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    let live = async {
        let balance = fetch_live(client, endpoints, api_key).await?;
        to_snapshot(balance)
    };
    run_cached_fetch(
        cache,
        Duration::from_secs(LOCK_TIMEOUT_SECS),
        cache_ttl,
        live,
        |text| parse_cache(text).ok(),
        snapshot_json,
    )
    .await
}

fn serde_repr(snap: &NovitaSnapshot) -> serde_json::Value {
    serde_json::json!({
        "available": snap.available,
        "cash": snap.cash,
        "credit_limit": snap.credit_limit,
        "outstanding": snap.outstanding,
    })
}

/// The shared pipeline stores `to_json` output verbatim; `Value::to_string`
/// is the same compact encoding `serde_json::to_vec` produced before.
fn snapshot_json(snap: &NovitaSnapshot) -> String {
    serde_json::json!({ "snapshot": serde_repr(snap) }).to_string()
}

fn parse_cache(text: &str) -> Result<NovitaSnapshot> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("novita cache missing 'snapshot' field".into()))?;
    // Cached money is required, not optional: a truncated or half-written
    // payload must be refetched rather than rendered as $0.00.
    let field = |name: &str| -> Result<f64> {
        let v = s[name]
            .as_f64()
            .ok_or_else(|| AppError::Schema(format!("novita cache missing '{name}'")))?;
        finite_amount("novita cache", name, v)
    };
    Ok(NovitaSnapshot {
        available: field("available")?,
        cash: field("cash")?,
        credit_limit: field("credit_limit")?,
        outstanding: field("outstanding")?,
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    api_key: &str,
) -> Result<BalanceData> {
    let resp = tokio::time::timeout(
        Duration::from_secs(HTTP_TIMEOUT_DEFAULT_SECS),
        client
            .get(&endpoints.balance)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("novita timeout: {}", endpoints.balance)))??;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let data: BalanceData = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("novita {}: {e}", endpoints.balance)))?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("novita"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    /// The other half of the `401`-body fix. Deepseek's test covers the vendors
    /// that passed the pair inline; these six built it into a `diag` local
    /// first, which is why the original sweep missed them — same defect, one
    /// variable name apart.
    #[tokio::test]
    async fn a_401_body_does_not_reach_the_outcome_when_a_cache_is_warm() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(401)
            .with_body("NOVITA user@example.test <credential>&token")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let warm = serde_json::json!({
            "snapshot": {
                "available": 10.0,
                "cash": 8.0,
                "credit_limit": 2.0,
                "outstanding": 0.0
            }
        });
        cache.write_payload(warm.to_string().as_bytes()).unwrap();

        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "nv-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .expect("a warm cache must still produce an outcome");

        let (code, msg) = out.last_error.expect("the 401 must still be reported");
        assert_eq!(code, 401);
        assert_eq!(msg, crate::error::AUTH_FAILURE_MESSAGE);
        assert!(!msg.contains("NOVITA"), "{msg}");
        assert!(!msg.contains("<credential>"), "{msg}");
    }

    #[tokio::test]
    async fn live_fetch_parses_available_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .match_header("authorization", "Bearer nv-test")
            .with_status(200)
            .with_body(
                r#"{"availableBalance":"1000000","cashBalance":"800000",
                    "creditLimit":"200000","pendingCharges":"0","outstandingInvoices":"0"}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "nv-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.available, 100.0);
        assert_eq!(out.snapshot.cash, 80.0);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn http_error_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seed = serde_json::json!({ "snapshot": {
            "available": 42.0, "cash": 40.0, "credit_limit": 20.0, "outstanding": 0.0
        }});
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(0))
            .await
            .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.available, 42.0);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }

    #[tokio::test]
    async fn malformed_200_body_does_not_become_a_zero_balance() {
        // Novita answering 200 with an error envelope must surface as a schema
        // error, never as a fresh "$0.00 available" snapshot.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(200)
            .with_body(r#"{"code":401,"message":"invalid api key"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(0)).await;
        assert!(out.is_err(), "expected a schema error, got {out:?}");
    }

    #[tokio::test]
    async fn corrupt_cache_is_not_served_as_zero() {
        // A truncated payload must be refetched, not rendered as $0.00.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(200)
            .with_body(
                r#"{"availableBalance":"55000","cashBalance":"55000",
                    "creditLimit":"0","outstandingInvoices":"0"}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        // Fresh, but missing the monetary fields.
        let seed = serde_json::json!({ "snapshot": { "available": 42.0 } });
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(out.snapshot.available, 5.5);
        assert!(!out.stale);
    }
}
