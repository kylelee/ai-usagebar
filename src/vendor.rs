//! Shared vendor IDs and renderer/fetcher structs used by the widget and TUI.
//!
//! Snapshots remain a discriminated `VendorSnapshot` enum because the vendors
//! have genuinely different shapes — see `usage.rs`.

use std::time::Duration;

use clap::ValueEnum;

use crate::format::substitute;
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, severity_color};
use crate::theme::Theme;
use crate::usage::VendorSnapshot;
use crate::waybar::{Class, WaybarOutput};

/// Outer reqwest client timeout shared by widget and TUI entry points.
/// Vendor fetchers still apply their own tighter per-request timeouts.
pub const HTTP_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on a vendor response body. Every one of these endpoints returns
/// a small JSON document — the largest observed is a few kilobytes — so this is
/// generous by three orders of magnitude while still bounding the damage from a
/// misbehaving proxy or a hijacked endpoint.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Credential-bearing environment variables owned by ai-usagebar vendors.
/// Subprocesses receive only the entries that belong to their own provider.
pub(crate) const VENDOR_SECRET_ENV_VARS: &[&str] = &[
    "ZAI_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "KIMI_API_KEY",
    "KILO_API_KEY",
    "NOVITA_API_KEY",
    "MINIMAX_API_KEY",
    "MOONSHOT_API_KEY",
    "XAI_MANAGEMENT_KEY",
    "ANTHROPIC_ADMIN_KEY",
    "OPENAI_ADMIN_KEY",
    "XAI_API_KEY",
    "GROK_API_KEY",
    "OPENCODE_GO_API_KEY",
];

pub(crate) fn vendor_secret_env_vars_to_remove(keep: &[&str]) -> Vec<&'static str> {
    VENDOR_SECRET_ENV_VARS
        .iter()
        .copied()
        .filter(|var| !keep.contains(var))
        .collect()
}

/// Follow ordinary vendor redirects without forwarding non-standard API-key
/// headers to a different origin. Reqwest strips `Authorization` on sensitive
/// redirects, but vendors also use headers such as `x-api-key`, which are not
/// covered by that built-in list.
pub fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.stop();
        };
        let target = attempt.url();
        if target.scheme() == origin.scheme()
            && target.host_str() == origin.host_str()
            && target.port_or_known_default() == origin.port_or_known_default()
        {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// Read a response body with an upper bound.
///
/// Every vendor buffered the whole body with `resp.bytes()` *before* anything
/// validated it. The widget is re-executed by Waybar every 60s, so an endpoint
/// answering with an unbounded stream had a free hand at the machine's memory.
/// `Content-Length` is checked first when present, then the body is read in
/// chunks so a lying or absent length cannot get past the cap either.
pub async fn read_body_capped(
    mut resp: reqwest::Response,
    max: usize,
) -> crate::error::Result<Vec<u8>> {
    let too_big = |n: u64| {
        crate::error::AppError::Schema(format!(
            "response body exceeds the {max}-byte limit ({n} bytes); refusing to buffer it"
        ))
    };
    if let Some(len) = resp.content_length()
        && len > max as u64
    {
        return Err(too_big(len));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if chunk.len() > max.saturating_sub(buf.len()) {
            return Err(too_big(buf.len().saturating_add(chunk.len()) as u64));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Stable enum used by `--vendor` and in config files.
#[derive(
    Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum VendorId {
    Anthropic,
    #[serde(rename = "anthropic_api")]
    AnthropicApi,
    Openai,
    #[serde(rename = "openai_api")]
    OpenaiApi,
    Zai,
    Openrouter,
    Deepseek,
    Kimi,
    Kilo,
    Novita,
    Moonshot,
    Grok,
    Supergrok,
    Antigravity,
    Cursor,
    Minimax,
    Kiro,
    #[serde(rename = "opencode-go")]
    OpenCodeGo,
}

impl VendorId {
    pub fn slug(self) -> &'static str {
        match self {
            VendorId::Anthropic => "anthropic",
            VendorId::AnthropicApi => "anthropic_api",
            VendorId::Openai => "openai",
            VendorId::OpenaiApi => "openai_api",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "openrouter",
            VendorId::Deepseek => "deepseek",
            VendorId::Kimi => "kimi",
            VendorId::Kilo => "kilo",
            VendorId::Novita => "novita",
            VendorId::Moonshot => "moonshot",
            VendorId::Grok => "grok",
            VendorId::Supergrok => "supergrok",
            VendorId::Antigravity => "antigravity",
            VendorId::Cursor => "cursor",
            VendorId::Minimax => "minimax",
            VendorId::Kiro => "kiro",
            VendorId::OpenCodeGo => "opencode-go",
        }
    }

    /// Inverse of [`VendorId::slug`]: the single slug → vendor mapping. The
    /// active-vendor state file, report entry ids (`<slug>@<label>`), and any
    /// consumer that round-trips a slug go through here; an unknown, empty,
    /// or case-mismatched slug is `None` (slugs are lowercase by contract).
    pub fn from_slug(slug: &str) -> Option<VendorId> {
        Some(match slug {
            "anthropic" => VendorId::Anthropic,
            "anthropic_api" => VendorId::AnthropicApi,
            "openai" => VendorId::Openai,
            "openai_api" => VendorId::OpenaiApi,
            "zai" => VendorId::Zai,
            "openrouter" => VendorId::Openrouter,
            "deepseek" => VendorId::Deepseek,
            "kimi" => VendorId::Kimi,
            "kilo" => VendorId::Kilo,
            "novita" => VendorId::Novita,
            "moonshot" => VendorId::Moonshot,
            "grok" => VendorId::Grok,
            "supergrok" => VendorId::Supergrok,
            "antigravity" => VendorId::Antigravity,
            "cursor" => VendorId::Cursor,
            "minimax" => VendorId::Minimax,
            "kiro" => VendorId::Kiro,
            "opencode-go" => VendorId::OpenCodeGo,
            _ => return None,
        })
    }

    /// Canonical human-readable name for shared reports and compact UI labels.
    /// Platform frontends may add context (for example, "GLM (Z.AI)" in a
    /// wide TUI tab), but should not carry their own full vendor-name table.
    pub fn display_name(self) -> &'static str {
        match self {
            VendorId::Anthropic => "Claude",
            VendorId::AnthropicApi => "Anthropic API",
            VendorId::Openai => "Codex",
            VendorId::OpenaiApi => "OpenAI API",
            VendorId::Zai => "Z.AI",
            VendorId::Openrouter => "OpenRouter",
            VendorId::Deepseek => "DeepSeek",
            VendorId::Kimi => "Kimi",
            VendorId::Kilo => "Kilo",
            VendorId::Novita => "Novita",
            VendorId::Moonshot => "Moonshot",
            VendorId::Grok => "Grok",
            VendorId::Supergrok => "SuperGrok",
            VendorId::Antigravity => "Antigravity",
            VendorId::Cursor => "Cursor",
            VendorId::Minimax => "MiniMax",
            VendorId::Kiro => "Kiro",
            VendorId::OpenCodeGo => "OpenCode Go",
        }
    }

    /// Compact three-letter code for the bar. This is the single source for
    /// `{vendor_short}` in every renderer, the `usage --json` `short_name`
    /// field, and any frontend that wants a Waybar-style provider tag; a
    /// second copy in a placeholder map or a QML file is how the table forks.
    pub const fn short_name(self) -> &'static str {
        match self {
            VendorId::Anthropic => "cld",
            VendorId::AnthropicApi => "aac",
            VendorId::Openai => "gpt",
            VendorId::OpenaiApi => "oai",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "opr",
            VendorId::Deepseek => "dsk",
            VendorId::Kimi => "kmi",
            VendorId::Kilo => "klo",
            VendorId::Novita => "nvt",
            VendorId::Moonshot => "msh",
            VendorId::Grok => "grk",
            VendorId::Supergrok => "sgk",
            VendorId::Antigravity => "agy",
            VendorId::Cursor => "cur",
            VendorId::Minimax => "mmx",
            VendorId::Kiro => "kir",
            VendorId::OpenCodeGo => "ocg",
        }
    }

    /// Asset id for the vendor's logo — the shared key every graphical
    /// frontend resolves against its bundled icons (`assets/logos/<slug>.svg`).
    /// Same brand intentionally shares one asset: Grok and SuperGrok are both
    /// xAI's mark, Anthropic's API product is the Anthropic glyph. Frontends
    /// fall back to the text tag when the asset is absent; this table stays
    /// the single mapping so the bar and the tabs can never disagree.
    pub const fn logo_slug(self) -> &'static str {
        match self {
            VendorId::Anthropic => "claude",
            VendorId::AnthropicApi => "anthropic",
            VendorId::Openai | VendorId::OpenaiApi => "openai",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "openrouter",
            VendorId::Deepseek => "deepseek",
            VendorId::Kimi => "kimi",
            VendorId::Kilo => "kilo",
            VendorId::Novita => "novita",
            VendorId::Moonshot => "moonshot",
            VendorId::Grok | VendorId::Supergrok => "grok",
            VendorId::Antigravity => "antigravity",
            VendorId::Cursor => "cursor",
            VendorId::Minimax => "minimax",
            VendorId::Kiro => "kiro",
            VendorId::OpenCodeGo => "opencode",
        }
    }

    pub fn all() -> &'static [VendorId] {
        &[
            VendorId::Anthropic,
            VendorId::AnthropicApi,
            VendorId::Openai,
            VendorId::OpenaiApi,
            VendorId::Zai,
            VendorId::Openrouter,
            VendorId::Deepseek,
            VendorId::Kimi,
            VendorId::Kilo,
            VendorId::Novita,
            VendorId::Moonshot,
            VendorId::Grok,
            VendorId::Supergrok,
            VendorId::Antigravity,
            VendorId::Cursor,
            VendorId::Minimax,
            VendorId::Kiro,
            VendorId::OpenCodeGo,
        ]
    }
}

/// What a vendor returns from a successful fetch — snapshot + meta. Mirrors
/// `anthropic::fetch::FetchOutcome` but vendor-agnostic.
#[derive(Debug, Clone)]
pub struct VendorOutcome {
    pub snapshot: VendorSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<std::time::Duration>,
}

/// A vendor fetch result that can be split into the vendor-agnostic
/// [`VendorOutcome`] plus the typed snapshot its renderer takes — the pair
/// every `widget::run` dispatch needs. Implemented for every vendor's
/// `fetch::FetchOutcome` so the widget's shared
/// fetch → transient-fallback → render pipeline can replace a per-vendor
/// copy of the same ten lines.
pub trait VendorFetchOutcome: Sized + Into<VendorOutcome> {
    /// The typed snapshot this vendor's `vendor::render` expects.
    type Snapshot: Clone;
    /// Borrow the typed snapshot while the outcome is still intact.
    fn typed_snapshot(&self) -> &Self::Snapshot;
    /// Split into the vendor-agnostic outcome and the typed snapshot. The
    /// snapshot is cloned first because `Into<VendorOutcome>` consumes
    /// `self` whole.
    fn into_parts(self) -> (VendorOutcome, Self::Snapshot) {
        let snapshot = self.typed_snapshot().clone();
        (self.into(), snapshot)
    }
}

macro_rules! impl_vendor_fetch_outcome {
    ($($outcome:path => $snapshot:ty),* $(,)?) => {
        $(
            impl VendorFetchOutcome for $outcome {
                type Snapshot = $snapshot;
                fn typed_snapshot(&self) -> &Self::Snapshot {
                    &self.snapshot
                }
            }
        )*
    };
}

impl_vendor_fetch_outcome! {
    crate::anthropic_api::fetch::FetchOutcome => crate::usage::AnthropicApiSnapshot,
    crate::antigravity::fetch::FetchOutcome => crate::usage::AntigravitySnapshot,
    crate::cursor::fetch::FetchOutcome => crate::usage::CursorSnapshot,
    crate::deepseek::fetch::FetchOutcome => crate::usage::DeepseekSnapshot,
    crate::grok::fetch::FetchOutcome => crate::usage::GrokSnapshot,
    crate::kilo::fetch::FetchOutcome => crate::usage::KiloSnapshot,
    crate::kimi::fetch::FetchOutcome => crate::usage::KimiSnapshot,
    crate::kiro::fetch::FetchOutcome => crate::usage::KiroSnapshot,
    crate::minimax::fetch::FetchOutcome => crate::usage::MinimaxSnapshot,
    crate::moonshot::fetch::FetchOutcome => crate::usage::MoonshotSnapshot,
    crate::novita::fetch::FetchOutcome => crate::usage::NovitaSnapshot,
    crate::openai::fetch::FetchOutcome => crate::usage::OpenAiSnapshot,
    crate::openai_api::fetch::FetchOutcome => crate::usage::OpenAiApiSnapshot,
    crate::opencode_go::fetch::FetchOutcome => crate::opencode_go::types::Usage,
    crate::openrouter::fetch::FetchOutcome => crate::usage::OpenRouterSnapshot,
    crate::supergrok::fetch::FetchOutcome => crate::usage::SuperGrokSnapshot,
    crate::zai::fetch::FetchOutcome => crate::usage::ZaiSnapshot,
}

/// Options forwarded to renderers from the CLI.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub format: Option<String>,
    pub tooltip_format: Option<String>,
    pub icon: Option<String>,
    pub pace_tolerance: u32,
    pub format_pace_color: bool,
    pub tooltip_pace_pts: bool,
}

/// Shared Waybar render shell: the pipeline every vendor's `render()` repeats,
/// ported byte-for-byte from the canonical `openrouter::vendor::render`
/// (src/openrouter/vendor.rs:68-110). Order, and what it means:
///   1. resolve the format — `--format` wins, else the vendor's default;
///   2. rewrite claudebar's `${token}` spelling to `{token}`;
///   3. substitute the placeholder map (unknown keys pass through);
///   4. append `" ⏸"` (leading space) when the outcome is stale;
///   5. wrap icon + text in a `<span>` colored by the severity tier;
///   6. tooltip = substituted `--tooltip-format`, else `default_tooltip`.
///
/// `values` is substituted VERBATIM — no Pango escaping happens here.
/// Placeholders that can carry untrusted text must be pre-escaped inside the
/// vendor's `build_placeholders` (the render-sink rule).
///
/// `severity = None` renders unwrapped (uncolored) text with the neutral
/// `Class::Low`; real vendors always pass their snapshot's severity.
pub fn render_shell(
    default_format: &str,
    values: &std::collections::HashMap<&str, String>,
    opts: &RenderOpts,
    stale: bool,
    severity: Option<PaceSeverity>,
    theme: &Theme,
    default_tooltip: String,
) -> WaybarOutput {
    let class = Class::from(severity.unwrap_or(PaceSeverity::Low));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| default_format.to_string());
    let format_clean = format.replace("${", "{");
    let mut text = substitute(&format_clean, values);
    if stale {
        text.push_str(" ⏸");
    }
    let icon_prefix = match opts.icon.as_deref() {
        Some(ic) if !ic.is_empty() => format!("{ic} "),
        _ => String::new(),
    };
    let bar_text = match severity {
        Some(sev) => color_span(severity_color(sev, theme), &format!("{icon_prefix}{text}")),
        None => format!("{icon_prefix}{text}"),
    };
    let tooltip = if let Some(fmt) = opts.tooltip_format.as_deref() {
        substitute(&fmt.replace("${", "{"), values)
    } else {
        default_tooltip
    };
    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vendor_has_stable_machine_and_display_names() {
        for vendor in VendorId::all() {
            assert!(!vendor.slug().is_empty());
            assert!(!vendor.display_name().is_empty());
        }
        assert_eq!(VendorId::Anthropic.slug(), "anthropic");
        assert_eq!(VendorId::Anthropic.display_name(), "Claude");
        assert_eq!(VendorId::Openai.display_name(), "Codex");
        assert_eq!(VendorId::Zai.display_name(), "Z.AI");
    }

    /// `{vendor_short}` is a documented format placeholder and now also rides
    /// the `usage --json` report, so a duplicate or a re-typed code would make
    /// two providers indistinguishable in a bar that shows nothing else.
    #[test]
    fn every_vendor_short_name_is_a_unique_three_letter_code() {
        let mut seen = std::collections::BTreeSet::new();
        for vendor in VendorId::all() {
            let short = vendor.short_name();
            assert_eq!(short.len(), 3, "{} is not three letters", vendor.slug());
            assert!(
                short.chars().all(|c| c.is_ascii_lowercase()),
                "{} is not lowercase ascii",
                vendor.slug()
            );
            assert!(seen.insert(short), "{short} is used by two vendors");
        }
        assert_eq!(VendorId::Anthropic.short_name(), "cld");
        assert_eq!(VendorId::Openai.short_name(), "gpt");
        assert_eq!(VendorId::Zai.short_name(), "zai");
        assert_eq!(VendorId::Antigravity.short_name(), "agy");
    }

    #[test]
    fn new_vendor_contracts_keep_public_names_and_slugs() {
        assert_eq!(VendorId::OpenCodeGo.slug(), "opencode-go");
        assert_eq!(VendorId::OpenCodeGo.display_name(), "OpenCode Go");
        assert_eq!(
            serde_json::to_value(VendorId::OpenCodeGo).unwrap(),
            serde_json::json!("opencode-go")
        );
    }

    #[test]
    fn vendor_secret_env_vars_cover_config_defaults() {
        let configured_defaults = [
            "ZAI_API_KEY",
            "OPENROUTER_API_KEY",
            "DEEPSEEK_API_KEY",
            "KIMI_API_KEY",
            "KILO_API_KEY",
            "NOVITA_API_KEY",
            "MINIMAX_API_KEY",
            "MOONSHOT_API_KEY",
            "XAI_MANAGEMENT_KEY",
            "ANTHROPIC_ADMIN_KEY",
        ];
        for name in configured_defaults {
            assert!(VENDOR_SECRET_ENV_VARS.contains(&name), "missing {name}");
        }
    }

    #[test]
    fn vars_to_remove_preserves_only_requested_grok_credentials() {
        let removed = vendor_secret_env_vars_to_remove(&["XAI_API_KEY", "GROK_API_KEY"]);
        assert!(!removed.contains(&"XAI_API_KEY"));
        assert!(!removed.contains(&"GROK_API_KEY"));
        assert!(removed.contains(&"ANTHROPIC_ADMIN_KEY"));
        assert!(removed.contains(&"OPENROUTER_API_KEY"));
        assert_eq!(removed.len(), VENDOR_SECRET_ENV_VARS.len() - 2);
    }

    #[tokio::test]
    async fn body_over_the_cap_is_refused_and_under_it_round_trips() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/big")
            .with_status(200)
            .with_body("x".repeat(4096))
            .create_async()
            .await;
        server
            .mock("GET", "/small")
            .with_status(200)
            .with_body("hello")
            .create_async()
            .await;

        let client = reqwest::Client::new();

        // Over the cap: refused rather than buffered.
        let resp = client
            .get(format!("{}/big", server.url()))
            .send()
            .await
            .unwrap();
        let err = read_body_capped(resp, 1024).await.unwrap_err();
        assert!(
            err.to_string().contains("exceeds"),
            "unexpected error: {err}"
        );

        // Under the cap: identical to the previous `resp.bytes()` behaviour.
        let resp = client
            .get(format!("{}/small", server.url()))
            .send()
            .await
            .unwrap();
        assert_eq!(read_body_capped(resp, 1024).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn chunked_body_without_content_length_still_hits_the_cap() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/chunked")
            .with_status(200)
            .with_chunked_body(|writer| writer.write_all(&[b'x'; 4096]))
            .create_async()
            .await;

        let response = reqwest::Client::new()
            .get(format!("{}/chunked", server.url()))
            .send()
            .await
            .unwrap();
        assert!(response.content_length().is_none());
        let error = read_body_capped(response, 1024).await.unwrap_err();
        assert!(error.to_string().contains("exceeds"), "{error}");
    }

    #[tokio::test]
    async fn same_origin_redirects_still_work_with_vendor_headers() {
        let mut server = mockito::Server::new_async().await;
        let redirect = server
            .mock("GET", "/start")
            .match_header("x-api-key", "secret")
            .with_status(302)
            .with_header("location", "/finish")
            .create_async()
            .await;
        let finish = server
            .mock("GET", "/finish")
            .match_header("x-api-key", "secret")
            .with_status(200)
            .create_async()
            .await;
        let client = reqwest::Client::builder()
            .redirect(same_origin_redirect_policy())
            .build()
            .unwrap();

        let response = client
            .get(format!("{}/start", server.url()))
            .header("x-api-key", "secret")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        redirect.assert_async().await;
        finish.assert_async().await;
    }

    #[tokio::test]
    async fn cross_origin_redirects_are_not_followed_with_vendor_headers() {
        let mut origin = mockito::Server::new_async().await;
        let mut target = mockito::Server::new_async().await;
        let target_url = format!("{}/capture", target.url());
        let redirect = origin
            .mock("GET", "/start")
            .match_header("x-api-key", "secret")
            .with_status(302)
            .with_header("location", &target_url)
            .create_async()
            .await;
        let capture = target
            .mock("GET", "/capture")
            .expect(0)
            .create_async()
            .await;
        let client = reqwest::Client::builder()
            .redirect(same_origin_redirect_policy())
            .build()
            .unwrap();

        let response = client
            .get(format!("{}/start", origin.url()))
            .header("x-api-key", "secret")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        redirect.assert_async().await;
        capture.assert_async().await;
    }

    #[test]
    fn vendor_id_slug_round_trip() {
        for id in VendorId::all() {
            assert_eq!(
                id.slug(),
                serde_json::to_value(id).unwrap().as_str().unwrap()
            );
        }
    }

    #[test]
    fn from_slug_round_trips_every_vendor() {
        for id in VendorId::all() {
            assert_eq!(VendorId::from_slug(id.slug()), Some(*id));
        }
    }

    #[test]
    fn from_slug_rejects_unknown_empty_and_case_mismatches() {
        assert_eq!(VendorId::from_slug(""), None);
        assert_eq!(VendorId::from_slug("not-a-vendor"), None);
        assert_eq!(VendorId::from_slug("ANTHROPIC"), None);
        assert_eq!(VendorId::from_slug("Anthropic"), None);
    }

    /// Every logo id resolves to exactly one bundled asset, and same-brand
    /// vendors share one deliberately: Grok/SuperGrok are one asset, so the
    /// distinct set is one smaller than the vendor list — but two different
    /// brands must never collide on one icon.
    #[test]
    fn every_vendor_logo_slug_is_unique_per_brand() {
        let mut seen = std::collections::BTreeSet::new();
        let mut vendors = 0;
        for vendor in VendorId::all() {
            vendors += 1;
            let logo = vendor.logo_slug();
            assert!(!logo.is_empty(), "{} has no logo asset id", vendor.slug());
            assert!(
                logo.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{logo} is not a slug"
            );
            seen.insert(logo);
        }
        assert_eq!(
            seen.len(),
            vendors - 2,
            "exactly the same-brand pairs (Grok/SuperGrok, OpenAI/OpenAI API)              share an asset"
        );
        assert_eq!(VendorId::Grok.logo_slug(), VendorId::Supergrok.logo_slug());
        assert_eq!(
            VendorId::Openai.logo_slug(),
            VendorId::OpenaiApi.logo_slug()
        );
        assert_eq!(VendorId::Anthropic.logo_slug(), "claude");
        assert_eq!(VendorId::Antigravity.logo_slug(), "antigravity");
    }

    fn shell_opts() -> RenderOpts {
        RenderOpts {
            format: None,
            tooltip_format: None,
            icon: None,
            pace_tolerance: 5,
            format_pace_color: false,
            tooltip_pace_pts: false,
        }
    }

    fn shell_values() -> std::collections::HashMap<&'static str, String> {
        crate::format::placeholders(vec![("or_balance", "$74.50"), ("or_used_today", "$1.00")])
    }

    /// Hand-replicates openrouter's steps (default `${…}` format → `${`→`{` →
    /// substitute → severity span) for the fixture above; the expected string
    /// is written out byte by byte, never produced by a vendor render fn.
    #[test]
    fn default_format_render_matches_the_hand_built_expectation() {
        let out = render_shell(
            "${or_balance} · ${or_used_today}",
            &shell_values(),
            &shell_opts(),
            false,
            Some(PaceSeverity::Low),
            &Theme::default(),
            "TOOLTIP".into(),
        );
        // One Dark green = theme.green (theme.rs ONE_DARK_GREEN).
        assert_eq!(out.text, "<span foreground='#98c379'>$74.50 · $1.00</span>");
        assert_eq!(out.tooltip, "TOOLTIP");
        assert_eq!(out.class, Class::Low);
    }

    #[test]
    fn stale_appends_the_pause_glyph_with_a_leading_space() {
        let out = render_shell(
            "{or_balance} · {or_used_today}",
            &shell_values(),
            &shell_opts(),
            true,
            Some(PaceSeverity::Low),
            &Theme::default(),
            "TOOLTIP".into(),
        );
        assert_eq!(
            out.text,
            "<span foreground='#98c379'>$74.50 · $1.00 ⏸</span>"
        );
    }

    #[test]
    fn custom_icon_is_prefixed_inside_the_color_span() {
        let mut o = shell_opts();
        o.icon = Some("󱙺".into());
        let out = render_shell(
            "{or_balance}",
            &shell_values(),
            &o,
            false,
            Some(PaceSeverity::Low),
            &Theme::default(),
            "TOOLTIP".into(),
        );
        assert_eq!(out.text, "<span foreground='#98c379'>󱙺 $74.50</span>");
    }

    /// Port fidelity: an empty `--icon` must not add a lone space.
    #[test]
    fn empty_icon_adds_no_prefix() {
        let mut o = shell_opts();
        o.icon = Some(String::new());
        let out = render_shell(
            "{or_balance}",
            &shell_values(),
            &o,
            false,
            Some(PaceSeverity::Low),
            &Theme::default(),
            "TOOLTIP".into(),
        );
        assert_eq!(out.text, "<span foreground='#98c379'>$74.50</span>");
    }

    #[test]
    fn custom_tooltip_format_beats_the_default_tooltip() {
        let mut o = shell_opts();
        o.tooltip_format = Some("bal: ${or_balance}".into());
        let out = render_shell(
            "{or_balance}",
            &shell_values(),
            &o,
            false,
            Some(PaceSeverity::Low),
            &Theme::default(),
            "DEFAULT TOOLTIP".into(),
        );
        assert_eq!(out.tooltip, "bal: $74.50");
    }

    /// Cross-check against the string openrouter's own suite pins byte for
    /// byte in `openrouter::vendor::tests::custom_tooltip_uses_placeholders`
    /// ("bal: $74.50 | mtd: $25.50" for these literal values).
    #[test]
    fn cross_checks_openrouter_pinned_tooltip_byte_for_byte() {
        let values = crate::format::placeholders(vec![
            ("or_balance", "$74.50"),
            ("or_used_month", "$25.50"),
        ]);
        let mut o = shell_opts();
        o.tooltip_format = Some("bal: {or_balance} | mtd: {or_used_month}".into());
        let out = render_shell(
            "",
            &values,
            &o,
            false,
            None,
            &Theme::default(),
            "UNUSED".into(),
        );
        assert_eq!(out.tooltip, "bal: $74.50 | mtd: $25.50");
    }

    /// claudebar's `${or_balance}` spelling works in user-supplied formats.
    #[test]
    fn claudebar_dollar_brace_tokens_still_substitute() {
        let mut o = shell_opts();
        o.format = Some("${or_balance} remaining".into());
        let out = render_shell(
            "{or_balance}",
            &shell_values(),
            &o,
            false,
            Some(PaceSeverity::Low),
            &Theme::default(),
            "TOOLTIP".into(),
        );
        assert_eq!(
            out.text,
            "<span foreground='#98c379'>$74.50 remaining</span>"
        );
    }

    /// Contract boundary: render_shell never escapes. Values that can carry
    /// untrusted text are pre-escaped in each vendor's build_placeholders.
    #[test]
    fn values_are_substituted_verbatim_escaping_belongs_to_the_caller() {
        let values = crate::format::placeholders(vec![("or_label", "<b>&raw</b>")]);
        let out = render_shell(
            "{or_label}",
            &values,
            &shell_opts(),
            false,
            None,
            &Theme::default(),
            String::new(),
        );
        assert_eq!(out.text, "<b>&raw</b>");
    }

    #[test]
    fn unknown_placeholders_pass_through_literally() {
        let out = render_shell(
            "{nope}",
            &std::collections::HashMap::new(),
            &shell_opts(),
            false,
            None,
            &Theme::default(),
            String::new(),
        );
        assert_eq!(out.text, "{nope}");
    }

    #[test]
    fn no_severity_renders_uncolored_text() {
        let out = render_shell(
            "{or_balance}",
            &shell_values(),
            &shell_opts(),
            false,
            None,
            &Theme::default(),
            "TOOLTIP".into(),
        );
        assert_eq!(out.text, "$74.50");
        assert_eq!(out.class, Class::Low);
    }

    #[test]
    fn class_follows_the_severity_tier() {
        let out = render_shell(
            "{or_balance}",
            &shell_values(),
            &shell_opts(),
            false,
            Some(PaceSeverity::Critical),
            &Theme::default(),
            "TOOLTIP".into(),
        );
        // One Dark red = theme.red (theme.rs ONE_DARK_RED).
        assert_eq!(out.text, "<span foreground='#e06c75'>$74.50</span>");
        assert_eq!(out.class, Class::Critical);
    }
}
