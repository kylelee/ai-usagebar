//! MiniMax Token Plan vendor — subscription quota from
//! `/v1/token_plan/remains` over an API key. The key is instance-scoped:
//! `api.minimax.io` (global) and `api.minimax.cn` (CN) reject each other's
//! keys, so the region is configured — with one sibling retry on rejection,
//! in [`fetch`], before the fetch is declared an auth failure.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
