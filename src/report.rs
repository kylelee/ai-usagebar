//! `ai-usagebar-omarchy usage` — quota and time-to-reset for everything in the config,
//! in one pass.
//!
//! The widget answers "how is *this* vendor doing" one process at a time, which
//! is what a status bar needs and what a person checking on four Claude
//! accounts does not. This walks the same tab set the TUI builds — every
//! enabled vendor, plus one entry per named Claude account — and prints what
//! each one has left.
//!
//! Deliberately thin: [`crate::tabs::tabs_from_config`] already decides
//! what is configured, [`crate::tabs::refresh_one`] already fetches and
//! parses it, and [`crate::tabs::sections_with_metadata_for`] already
//! projects any vendor's snapshot into labelled sections carrying every
//! reported value. So this file only enumerates, projects, and formats —
//! no vendor ever needs to know it exists.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::pacing::{self, PaceSeverity};
use crate::tabs::{
    Section, TabId, TabState, refresh_one, sections_with_metadata_for, tabs_with_desktop,
};

/// Matches the widget's `--pace-tolerance` default; only affects the pacing
/// note appended to a metric's detail line.
const PACE_TOLERANCE: u32 = pacing::DEFAULT_TOLERANCE;

/// One configured vendor or account.
#[derive(Debug, Clone)]
struct Entry {
    id: String,
    name: String,
    display_name: String,
    /// The vendor's `{vendor_short}` code. Frontends that want a Waybar-style
    /// provider tag take it from here rather than keeping their own table.
    short_name: String,
    /// The vendor's logo asset id (`VendorId::logo_slug`) — the key a
    /// frontend resolves against its bundled icons. Frontends fall back to
    /// the text tag when they ship no asset for it.
    logo: String,
    plan: Option<String>,
    sections: Vec<ReportSection>,
    error: Option<String>,
    /// `true` when `error` is the routine "not configured yet" state (no
    /// key, no login) rather than a broken configuration. Frontends use it
    /// to keep the display calm: a hint, not an alarm.
    unconfigured: bool,
    /// `true` when every reported window is maxed or the balance is spent —
    /// the account has no capacity left RIGHT NOW. Status bars hide
    /// exhausted tiles (the tab and the popup keep them); recovery is
    /// automatic because the flag is recomputed from every fresh report.
    exhausted: bool,
    /// The precomputed tile headline (fullest metric / balance figure), so
    /// no frontend has to re-derive which figure leads. `None` for error
    /// entries and entries with nothing reportable.
    headline: Option<Headline>,
    stale: bool,
    fetched_at: Option<DateTime<Utc>>,
}

/// Classify a sanitized entry error as the benign not-configured state.
/// Matched against the remedy messages this codebase itself generates —
/// a new vendor's "absent credentials" wording must be added here (and to
/// the classification test) or it will render as an error again.
pub(crate) fn is_unconfigured_error(message: &str) -> bool {
    const MARKERS: &[&str] = &[
        // API-key vendors (config::resolve_api_key_in_section).
        "no API key. Either",
        // Anthropic / Codex / Kimi logins.
        "Claude: no credentials at",
        "Codex: no auth file at",
        "Kimi: no credentials. Either",
        // Cursor / Kiro local sessions.
        "sign in to the Cursor",
        "kiro-cli login",
        // Antigravity local server.
        "Antigravity is not running",
    ];
    MARKERS.iter().any(|marker| message.contains(marker))
}

/// Structured-first classification of an error value: `Unconfigured` is the
/// calm state by construction, whatever its wording. Everything else falls
/// back to the string MARKERS above — including every string-only context,
/// where the variant is already lost to `TabState::Error`'s `String` by the
/// time a report entry classifies it.
pub fn is_unconfigured_app_error(err: &crate::error::AppError) -> bool {
    match err {
        crate::error::AppError::Unconfigured(_) => true,
        other => is_unconfigured_error(&other.user_message()),
    }
}

/// A row that carries remaining prepaid capacity: its value going to (or
/// below) zero means the account has nothing left to spend. "Spend" rows are
/// deliberately absent — a spend of $0 is an untouched account, not a spent
/// one.
fn is_balance_row(label: &str) -> bool {
    let label = label.to_lowercase();
    ["balance", "available", "prepaid"]
        .iter()
        .any(|word| label.contains(word))
}

/// The headline pick shared by the report JSON, the aggregate bar tile, and
/// (as the precomputed `headline` field) every native frontend: the fullest
/// metric wins; with no metrics at all, the first balance-style text row
/// stands in. `None` when nothing qualifies — frontends render their own
/// terminal text ("Error"/"Ready") for that case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeadlinePick {
    Metric(usize),
    BalanceRow(usize),
}

/// The label words that make a TEXT row a headline candidate. `spend` is
/// deliberately included here but not in [`is_balance_row`]: a "$0 spend"
/// row is good news and must not mark an entry exhausted, yet it is still
/// the figure an API-spend vendor leads with.
fn is_headline_balance_row(label: &str) -> bool {
    let label = label.to_lowercase();
    ["balance", "available", "spend", "prepaid"]
        .iter()
        .any(|word| label.contains(word))
}

/// Select the headline row from projected `(label, percent, value)` triples.
/// Ties keep the FIRST fullest metric, matching every frontend's fallback.
pub(crate) fn headline_pick<'a>(
    rows: impl Iterator<Item = (&'a str, Option<u16>, &'a str)>,
) -> Option<HeadlinePick> {
    let mut best: Option<(usize, u16)> = None;
    let mut balance: Option<usize> = None;
    for (index, (label, percent, value)) in rows.enumerate() {
        if let Some(pct) = percent {
            if best.is_none_or(|(_, best_pct)| pct > best_pct) {
                best = Some((index, pct));
            }
        } else if balance.is_none() && !value.is_empty() && is_headline_balance_row(label) {
            balance = Some(index);
        }
    }
    best.map(|(index, _)| HeadlinePick::Metric(index))
        .or(balance.map(HeadlinePick::BalanceRow))
}

/// The precomputed headline a frontend's tile leads with — the report JSON's
/// `headline` field. Mirrors the local walk `omarchy/Model.js` and the KDE
/// `plasmoid-logic.mjs` keep as their older-binary fallback.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Headline {
    pub text: String,
    pub percent: Option<u16>,
    pub severity: String,
    pub label: String,
    pub reset_at: Option<DateTime<Utc>>,
}

/// Build the entry's [`Headline`] from its projected sections.
fn headline_for_sections(sections: &[ReportSection]) -> Option<Headline> {
    // Pick over the DENSE candidate list and index that same list. Spacers
    // and the title sit between the metric rows in `sections`, so feeding
    // `headline_pick` a filtered iterator while indexing the full array made
    // the precomputed headline describe a row *neighbouring* the winner (a
    // 25% session window led while the 40% weekly one was the fullest).
    let candidates: Vec<&ReportSection> = sections
        .iter()
        .filter(|section| {
            matches!(
                section,
                ReportSection::Metric { .. } | ReportSection::Text { .. }
            )
        })
        .collect();
    let pick = headline_pick(candidates.iter().map(|section| match section {
        ReportSection::Metric {
            label,
            percent,
            value,
            ..
        } => (label.as_str(), Some(*percent), value.as_str()),
        ReportSection::Text { label, value } => (label.as_str(), None, value.as_str()),
        _ => unreachable!("the filter admits only metric and text rows"),
    }))?;
    match pick {
        HeadlinePick::Metric(index) => {
            let ReportSection::Metric {
                label,
                percent,
                value,
                severity,
                reset_at,
                ..
            } = candidates[index]
            else {
                return None;
            };
            Some(Headline {
                // Same rule as every frontend's fallback: a balance-labelled
                // metric leads with its money value, not a percentage.
                text: if label.to_lowercase().contains("balance") && !value.is_empty() {
                    value.clone()
                } else {
                    format!("{percent}%")
                },
                percent: Some(*percent),
                severity: severity.clone(),
                label: label.clone(),
                reset_at: *reset_at,
            })
        }
        HeadlinePick::BalanceRow(index) => {
            let ReportSection::Text { label, value } = candidates[index] else {
                return None;
            };
            Some(Headline {
                text: value.clone(),
                percent: None,
                severity: PaceSeverity::Low.as_str().into(),
                label: label.clone(),
                reset_at: None,
            })
        }
    }
}

/// Parse a rendered money value (`-$5.71`, `¥20.00`, `3.50 EUR`) back into a
/// number. `None` for anything that does not reduce to a plain decimal —
/// the caller only ever feeds it balance strings this project formatted.
fn balance_amount(value: &str) -> Option<f64> {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    if cleaned.is_empty() || cleaned == "-" || cleaned == "." {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Bar-tile exhaustion, the rule every frontend shares (the report ships it
/// as `exhausted`; the aggregate bar computes it directly): a tile leaves
/// the status bar once ANY reported percentage metric is at 100 — a maxed
/// window gates usage even when the others still have room — or a balance
/// row is at or below zero. Tabs and popups never filter on it.
pub(crate) fn rows_exhausted<'a>(
    rows: impl IntoIterator<Item = (&'a str, Option<u16>, &'a str)>,
) -> bool {
    for (label, percent, value) in rows {
        if percent.is_some_and(|pct| pct >= 100) {
            return true;
        }
        if is_balance_row(label)
            && let Some(amount) = balance_amount(value)
            && amount <= 0.0
        {
            return true;
        }
    }
    false
}

/// Lossless machine-readable projection of a TUI panel row. `metrics` remains
/// available in JSON as a convenience view over only the gauge rows; callers
/// that need every reported value should consume this ordered list.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReportSection {
    Metric {
        label: String,
        percent: u16,
        value: String,
        detail: String,
        severity: String,
        reset_at: Option<DateTime<Utc>>,
        /// Server-classified rolling window ("session" | "weekly") — the
        /// single source for bar-tile window picking on every frontend.
        window: Option<&'static str>,
        /// The window's length in seconds, when known. With `reset_at` it
        /// gives the fraction of the window's time still on the clock — the
        /// countdown figure's own color band, independent of `percent`.
        window_seconds: Option<i64>,
    },
    Text {
        label: String,
        value: String,
    },
    Block {
        label: String,
        body: Vec<String>,
    },
    Spacer,
}

impl ReportSection {
    fn label(&self) -> Option<&str> {
        match self {
            Self::Metric { label, .. } | Self::Text { label, .. } | Self::Block { label, .. } => {
                Some(label)
            }
            Self::Spacer => None,
        }
    }
}

pub async fn run(json: bool) -> i32 {
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ai-usagebar-omarchy usage: {}", error.user_message());
            return 1;
        }
    };
    let client = match crate::widget::run::http_client() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("ai-usagebar-omarchy usage: {}", error.user_message());
            return 1;
        }
    };

    let tabs = tabs_with_desktop(&config);
    if tabs.is_empty() {
        eprintln!(
            "ai-usagebar-omarchy usage: no vendors enabled in {}",
            crate::config::config_path_hint()
        );
        return 1;
    }

    // Sequential on purpose: several of these share a per-vendor cache lock,
    // and firing every account at Anthropic at once is a good way to get
    // rate-limited for no gain on a handful of entries.
    let mut entries = Vec::with_capacity(tabs.len());
    for tab in &tabs {
        entries.push(entry_for(&client, &config, tab).await);
    }
    apply_solo_display_names(&mut entries);

    if json {
        println!(
            "{}",
            render_json_for_primary(&entries, config.ui.primary.map(|vendor| vendor.slug()))
        );
    } else {
        print!("{}", render_text(&entries));
    }
    report_exit_code(&entries)
}

/// A vendor with a single entry ships that entry's `display_name` WITHOUT the
/// account suffix — "Kimi", not "Kimi · 1". The suffix exists to distinguish
/// siblings; a lone account has none, so every surface reading display_name
/// (the panel's tabs, hero, tooltips, the KDE plasmoid) shows the plain
/// provider name. Counted over the WHOLE report — an erroring or exhausted
/// sibling still exists, and when it reads again both names get their
/// suffixes back. The machine `name` keeps the suffix for stable addressing.
fn apply_solo_display_names(entries: &mut [Entry]) {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for entry in entries.iter() {
        let base = entry.id.split('@').next().unwrap_or("").to_string();
        *counts.entry(base).or_default() += 1;
    }
    for entry in entries.iter_mut() {
        if !entry.id.contains('@') {
            continue; // the vendor's default entry never carries a suffix
        }
        let base = entry.id.split('@').next().unwrap_or("");
        if counts.get(base).copied() != Some(1) {
            continue;
        }
        // The provider name comes from the id's vendor segment — never from
        // splitting display_name, where a label that itself contains " · "
        // would decide where the provider name ends.
        if let Some(vendor) = crate::vendor::VendorId::from_slug(base) {
            entry.display_name = vendor.display_name().to_string();
        }
    }
}

async fn entry_for(client: &reqwest::Client, config: &Config, tab: &TabId) -> Entry {
    let state = refresh_one(client, config, tab).await;
    entry_from_state(tab, &state, Utc::now())
}

fn entry_from_state(tab: &TabId, state: &TabState, now: chrono::DateTime<Utc>) -> Entry {
    let mut entry = Entry {
        id: tab_id(tab),
        name: tab_name(tab),
        display_name: tab_display_name(tab),
        short_name: tab.vendor.short_name().to_string(),
        logo: tab.vendor.logo_slug().to_string(),
        plan: None,
        sections: Vec::new(),
        error: match &state {
            TabState::Error(message) => Some(crate::display::sanitize_untrusted_field(message)),
            _ => None,
        },
        unconfigured: match &state {
            TabState::Error(message) => is_unconfigured_error(message),
            _ => false,
        },
        exhausted: false,
        headline: None,
        stale: matches!(state, TabState::Ready(ready) if ready.stale),
        fetched_at: match state {
            TabState::Ready(ready) => ready.fetched_at,
            _ => None,
        },
    };
    // The error is already a first-class entry field. Do not duplicate the
    // TUI's interactive retry instructions as report data.
    if entry.error.is_some() {
        return entry;
    }
    for projected in sections_with_metadata_for(state, now, PACE_TOLERANCE) {
        match projected.section {
            Section::Title { left, .. } => entry.plan = Some(left),
            Section::Metric {
                label,
                pct,
                value_label,
                footnote,
                severity,
                ..
            } => {
                entry.sections.push(ReportSection::Metric {
                    label: label.clone(),
                    percent: pct,
                    value: value_label.clone(),
                    detail: footnote,
                    severity: severity.as_str().into(),
                    reset_at: projected.reset_at,
                    window: projected.window,
                    window_seconds: projected.window_seconds,
                });
            }
            Section::Text { label, value } => {
                entry.sections.push(ReportSection::Text {
                    label: label.clone(),
                    value: value.clone(),
                });
            }
            Section::Block { label, body } => {
                entry.sections.push(ReportSection::Block { label, body });
            }
            Section::Spacer => entry.sections.push(ReportSection::Spacer),
        }
        // Exhaustion rides the same single pass: every projected metric and
        // balance row contributes, so the flag can never disagree with the
        // sections a frontend renders from.
        let projected_row = match entry.sections.last() {
            Some(ReportSection::Metric {
                label,
                percent,
                value,
                ..
            }) => Some((label.as_str(), Some(*percent), value.as_str())),
            Some(ReportSection::Text { label, value }) => {
                Some((label.as_str(), None, value.as_str()))
            }
            _ => None,
        };
        if !entry.exhausted
            && let Some(row) = projected_row
            && rows_exhausted(std::iter::once(row))
        {
            entry.exhausted = true;
        }
    }
    // The headline rides the same sections the frontends render from, so the
    // precomputed pick can never disagree with them.
    entry.headline = headline_for_sections(&entry.sections);
    entry
}

fn report_exit_code(entries: &[Entry]) -> i32 {
    i32::from(entries.iter().all(|entry| entry.error.is_some()))
}

/// Stable machine id shared by aggregate views and the macOS menu bar:
/// `<vendor>@<label>` for named accounts.
fn tab_id(tab: &TabId) -> String {
    match &tab.account {
        Some(account) => format!("{}@{account}", tab.vendor.slug()),
        None => tab.vendor.slug().to_string(),
    }
}

fn tab_name(tab: &TabId) -> String {
    format_tab_name(tab, tab.vendor.slug())
}

fn tab_display_name(tab: &TabId) -> String {
    format_tab_name(tab, tab.vendor.display_name())
}

fn format_tab_name(tab: &TabId, vendor_name: &str) -> String {
    let name = match &tab.account {
        // Mark a Desktop-sourced account so a mixed CLI+Desktop setup is legible;
        // for a Desktop-only user every Claude row simply reads "· <label> (desktop)".
        Some(account) if tab.desktop => format!("{vendor_name} · {account} (desktop)"),
        Some(account) => format!("{vendor_name} · {account}"),
        None => vendor_name.to_string(),
    };
    crate::display::sanitize_untrusted_field(&name)
}

fn render_json_for_primary(entries: &[Entry], primary: Option<&str>) -> String {
    let rows: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let metrics = entry
                .sections
                .iter()
                .filter_map(|section| match section {
                    ReportSection::Metric {
                        label,
                        percent,
                        value,
                        detail,
                        severity,
                        reset_at,
                        window,
                        window_seconds,
                    } => Some(json!({
                        "label": label,
                        "percent": percent,
                        "value": value,
                        "detail": detail,
                        "severity": severity,
                        "reset_at": reset_at,
                        "window": window,
                        "window_seconds": window_seconds,
                    })),
                    _ => None,
                })
                .collect::<Vec<_>>();
            json!({
                "id": entry.id,
                "name": entry.name,
                "display_name": entry.display_name,
                "short_name": entry.short_name,
                "logo": entry.logo,
                "plan": entry.plan,
                "status": if entry.error.is_some() { "error" } else { "ready" },
                "error": entry.error,
                "unconfigured": entry.unconfigured,
                "exhausted": entry.exhausted,
                "headline": entry.headline,
                "stale": entry.stale,
                "fetched_at": entry.fetched_at,
                "metrics": metrics,
                "sections": entry.sections,
            })
        })
        .collect();
    json!({ "primary": primary, "entries": rows }).to_string()
}

fn render_text(entries: &[Entry]) -> String {
    // Widest label across every entry, so the value column lines up down the
    // whole report rather than per-section.
    let width = entries
        .iter()
        .flat_map(|entry| entry.sections.iter())
        .filter_map(ReportSection::label)
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.name);
        if let Some(plan) = &entry.plan {
            out.push_str(&format!("   {plan}"));
        }
        out.push('\n');
        if let Some(error) = &entry.error {
            out.push_str(&format!("  ! {error}\n\n"));
            continue;
        }
        if !entry
            .sections
            .iter()
            .any(|section| !matches!(section, ReportSection::Spacer))
        {
            out.push_str("  (nothing reported)\n\n");
            continue;
        }
        let mut body = String::new();
        let mut pending_spacer = false;
        for section in &entry.sections {
            if matches!(section, ReportSection::Spacer) {
                pending_spacer |= !body.is_empty();
                continue;
            }
            if pending_spacer {
                body.push('\n');
                pending_spacer = false;
            }
            match section {
                ReportSection::Metric {
                    label,
                    value,
                    detail,
                    ..
                } => {
                    let label = format!("{label:width$}");
                    let value = format!("{value:>9}");
                    if detail.is_empty() {
                        body.push_str(&format!("  {label}  {value}\n"));
                    } else {
                        body.push_str(&format!("  {label}  {value}   {detail}\n"));
                    }
                }
                ReportSection::Text { label, value } => {
                    if label.is_empty() {
                        body.push_str(&format!("  {}\n", value.trim_start()));
                    } else if value.is_empty() {
                        body.push_str(&format!("  {label}\n"));
                    } else {
                        body.push_str(&format!("  {label:width$}  {value}\n"));
                    }
                }
                ReportSection::Block { label, body: lines } => {
                    body.push_str(&format!("  {label}\n"));
                    for line in lines {
                        body.push_str(&format!("    {line}\n"));
                    }
                }
                // Spacers are consumed by the pending_spacer logic above.
                ReportSection::Spacer => continue,
            }
        }
        out.push_str(&body);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use crate::tabs::ReadyTab;
    use crate::usage::{
        DeepseekSnapshot, KimiSnapshot, KiroSnapshot, MinimaxSnapshot, OpenRouterSnapshot,
        UsageWindow, VendorSnapshot,
    };
    use crate::vendor::VendorId;

    /// Routine absence is a hint, not an alarm; a broken configuration
    /// (bad key, half-configured team, schema drift) stays an error.
    #[test]
    fn unconfigured_classification_seeds_from_our_own_remedies() {
        for benign in [
            "credentials error: Zai: no API key. Either set an API key in a valid environment variable or set `api_key` under [zai] in …",
            "credentials error: OpenRouter: no API key. Either set an API key …",
            "credentials error: Claude: no credentials at /home/k/.claude/.credentials.json. Run `claude` once to sign in …",
            "credentials error: Codex: no auth file at /home/k/.codex/auth.json. Run `codex login` once …",
            "credentials error: Kimi: no credentials. Either log in with the Kimi Code CLI …",
        ] {
            assert!(is_unconfigured_error(benign), "should be calm: {benign}");
        }
        for alarming in [
            "HTTP 401 … unauthorized",
            "zai: a team account needs both `organization_id` and `project_id` …",
            "schema mismatch: …",
            "io error at … credentials.json: permission denied",
        ] {
            assert!(!is_unconfigured_error(alarming), "should alarm: {alarming}");
        }
    }

    #[test]
    fn unconfigured_variant_classifies_by_structure_not_wording() {
        let reworded = "brand new remedy prose — no marker this codebase emits";
        assert!(is_unconfigured_app_error(&AppError::Unconfigured(
            reworded.into()
        )));
        // The same text as a `Credentials` error stays an alarm: only the
        // variant is structural, the string path still needs a MARKERS hit.
        assert!(!is_unconfigured_app_error(&AppError::Credentials(
            reworded.into()
        )));
        // Non-credentials variants fall through to the string classifier.
        assert!(!is_unconfigured_app_error(&AppError::Transport(
            "connection refused".into()
        )));
        assert!(!is_unconfigured_app_error(&AppError::Http {
            status: 401,
            body: "unauthorized".into()
        }));
        // Malformed payloads change nothing: classification is variant-only,
        // and Display prints the inner text the producer chose.
        assert!(is_unconfigured_app_error(&AppError::Unconfigured(
            String::new()
        )));
        assert!(is_unconfigured_app_error(&AppError::Unconfigured(
            "\x1b[2Kspoof\n".into()
        )));
        // An empty inner still renders through the Credentials-mirroring
        // Display — no special case, no surprise.
        assert_eq!(
            AppError::Unconfigured(String::new()).to_string(),
            AppError::Credentials(String::new()).to_string()
        );
    }

    #[test]
    fn error_entries_carry_the_unconfigured_flag() {
        let tab = TabId::vendor(VendorId::Zai);
        let e = entry_from_state(
            &tab,
            &TabState::Error("credentials error: Zai: no API key. Either set an API key …".into()),
            Utc::now(),
        );
        assert!(e.unconfigured);
        assert!(e.error.is_some(), "the remedy text stays as the hint");
        let broken = entry_from_state(
            &tab,
            &TabState::Error("HTTP 401: unauthorized".into()),
            Utc::now(),
        );
        assert!(!broken.unconfigured);
    }

    fn entry(name: &str, sections: Vec<ReportSection>) -> Entry {
        let headline = headline_for_sections(&sections);
        Entry {
            id: name.into(),
            name: name.into(),
            display_name: name.into(),
            short_name: VendorId::Anthropic.short_name().into(),
            logo: VendorId::Anthropic.logo_slug().into(),
            plan: Some("Claude Max 20x".into()),
            sections,
            error: None,
            unconfigured: false,
            exhausted: false,
            headline,
            stale: false,
            fetched_at: None,
        }
    }

    fn metric(label: &str, percent: u16, value: &str, detail: &str) -> ReportSection {
        ReportSection::Metric {
            label: label.into(),
            percent,
            value: value.into(),
            detail: detail.into(),
            severity: "mid".into(),
            reset_at: None,
            window: None,
            window_seconds: None,
        }
    }

    #[test]
    fn accounts_get_a_stable_id_and_a_readable_name() {
        let account = TabId::account("gmail");
        assert_eq!(tab_id(&account), "anthropic@gmail");
        assert_eq!(tab_name(&account), "anthropic · gmail");
        assert_eq!(tab_display_name(&account), "Claude · gmail");

        let plain = TabId::vendor(VendorId::Cursor);
        assert_eq!(tab_id(&plain), "cursor");
        assert_eq!(tab_name(&plain), "cursor");
        assert_eq!(tab_display_name(&plain), "Cursor");

        let openrouter = TabId::account_for(VendorId::Openrouter, "work");
        assert_eq!(tab_id(&openrouter), "openrouter@work");
        assert_eq!(tab_name(&openrouter), "openrouter · work");
        assert_eq!(tab_display_name(&openrouter), "OpenRouter · work");
    }

    /// The Omarchy bar can be set to draw the provider tag and nothing else,
    /// so every entry — a failed one included — has to carry a code, and every
    /// account of one vendor has to carry that vendor's code rather than a
    /// per-account one.
    #[test]
    fn every_entry_carries_its_vendor_short_code() {
        let now = Utc::now();
        let failed = TabState::Error("not signed in".into());

        let account = entry_from_state(&TabId::account("gmail"), &failed, now);
        assert_eq!(account.short_name, "cld");
        let other_account = entry_from_state(&TabId::account("work"), &failed, now);
        assert_eq!(other_account.short_name, account.short_name);

        let cursor = entry_from_state(&TabId::vendor(VendorId::Cursor), &failed, now);
        assert_eq!(cursor.short_name, "cur");

        let rendered = render_json_for_primary(&[cursor], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["short_name"], "cur");
    }

    #[test]
    fn every_metric_reports_its_quota_and_its_reset() {
        let text = render_text(&[entry(
            "anthropic · gmail",
            vec![
                metric("Session (5h)", 29, "29%", "Resets in 0h 50m"),
                metric("Weekly (7d)", 32, "32%", "Resets in 4d 2h"),
            ],
        )]);

        assert!(
            text.contains("anthropic · gmail   Claude Max 20x"),
            "{text}"
        );
        assert!(text.contains("29%   Resets in 0h 50m"), "{text}");
        assert!(text.contains("32%   Resets in 4d 2h"), "{text}");
    }

    /// Labels are padded to one width across the whole report, so the columns
    /// still line up when a later entry has a longer label than the first.
    #[test]
    fn value_columns_align_across_entries() {
        let text = render_text(&[
            entry("a", vec![metric("S", 1, "1%", "")]),
            entry("b", vec![metric("A very long label", 2, "2%", "")]),
        ]);
        let columns: Vec<usize> = text
            .lines()
            .filter(|line| line.starts_with("  ") && line.contains('%'))
            .map(|line| line.find('%').unwrap())
            .collect();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0], columns[1], "{text}");
    }

    /// One dead vendor must not hide the others — it reports inline and the
    /// rest still print.
    #[test]
    fn a_failing_entry_is_reported_without_dropping_the_rest() {
        let mut broken = entry("openai", Vec::new());
        broken.error = Some("credentials error: not signed in".into());
        let text = render_text(&[broken, entry("cursor", vec![metric("Auto", 5, "5%", "")])]);

        assert!(
            text.contains("! credentials error: not signed in"),
            "{text}"
        );
        assert!(text.contains("cursor"), "{text}");
        assert!(text.contains("5%"), "{text}");
    }

    #[test]
    fn json_carries_the_percentage_as_a_number() {
        let rendered = render_json_for_primary(
            &[entry(
                "anthropic · gmail",
                vec![metric("Session (5h)", 29, "29%", "Resets in 0h 50m")],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["plan"], "Claude Max 20x");
        assert_eq!(first["display_name"], "anthropic · gmail");
        assert_eq!(first["short_name"], "cld");
        assert_eq!(first["metrics"][0]["percent"], 29);
        assert_eq!(first["metrics"][0]["detail"], "Resets in 0h 50m");
        assert!(first["error"].is_null());
        assert_eq!(first["status"], "ready");
        assert_eq!(first["stale"], false);
        assert!(first["fetched_at"].is_null());
        assert_eq!(first["metrics"][0]["severity"], "mid");
        assert!(first["metrics"][0]["reset_at"].is_null());
        assert!(value["primary"].is_null());
    }

    /// The precomputed `headline` field is what the native frontends' tiles
    /// lead with; the rule must match their local fallbacks exactly (see
    /// `omarchy/Model.js` and the KDE `plasmoid-logic.mjs`).
    #[test]
    fn json_precomputes_the_headline_figure() {
        // Balance-labelled metric → the money value leads, not a percentage.
        let rendered = render_json_for_primary(
            &[entry(
                "openrouter",
                vec![
                    metric("Credit balance", 87, "$1.34", "…"),
                    metric("Spend (30d)", 10, "10%", "…"),
                ],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let headline = &value["entries"][0]["headline"];
        assert_eq!(headline["text"], "$1.34");
        assert_eq!(headline["percent"], 87);
        assert_eq!(headline["label"], "Credit balance");

        // Quota metrics: the fullest window wins with its percentage.
        let rendered = render_json_for_primary(
            &[entry(
                "claude",
                vec![
                    metric("Session (5h)", 62, "62%", "…"),
                    metric("Weekly (7d)", 27, "27%", "…"),
                ],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let headline = &value["entries"][0]["headline"];
        assert_eq!(headline["text"], "62%");
        assert_eq!(headline["percent"], 62);
        assert_eq!(headline["severity"], "mid");
    }

    #[test]
    fn headline_pick_rules_match_the_frontend_fallbacks() {
        // No metrics: the first balance-style TEXT row stands in — "spend"
        // counts here even though it never marks an entry exhausted.
        let pick = headline_pick(
            [
                ("Vendor", None, "usage via API"),
                ("Spend (30d)", None, "$12.40"),
                ("Balance", None, "$4.00"),
            ]
            .into_iter(),
        );
        assert_eq!(pick, Some(HeadlinePick::BalanceRow(1)));

        // Any metric outranks text rows; ties keep the FIRST fullest.
        let pick = headline_pick(
            [
                ("Weekly", Some(40), "40%"),
                ("Session", Some(80), "80%"),
                ("Balance", None, "$4.00"),
            ]
            .into_iter(),
        );
        assert_eq!(pick, Some(HeadlinePick::Metric(1)));

        // Nothing reportable → no headline; the frontend renders "Ready".
        assert_eq!(headline_pick([("Vendor", None, "text")].into_iter()), None);
        // An empty balance text row is not a headline.
        assert_eq!(headline_pick([("Balance", None, "")].into_iter()), None);
    }

    #[test]
    fn json_carries_the_configured_primary_without_reordering_entries() {
        let rendered = render_json_for_primary(
            &[entry("anthropic", Vec::new()), entry("openai", Vec::new())],
            Some("openai"),
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["primary"], "openai");
        assert_eq!(value["entries"][0]["id"], "anthropic");
        assert_eq!(value["entries"][1]["id"], "openai");
    }

    #[test]
    fn json_exposes_absolute_resets_and_cache_freshness_additively() {
        let fetched_at = Utc::now() - chrono::Duration::minutes(3);
        let reset_at = Utc::now() + chrono::Duration::days(1);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Kiro(KiroSnapshot {
                plan: "KIRO POWER".into(),
                used: 4_000.0,
                limit: 10_000.0,
                reset_at: Some(reset_at),
            }),
            stale: true,
            last_error: None,
            fetched_at: Some(fetched_at),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Kiro), &state, Utc::now());
        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];

        assert_eq!(first["stale"], true);
        let fetched_rfc3339 = fetched_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        let reset_rfc3339 = reset_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        assert_eq!(first["fetched_at"], fetched_rfc3339);
        assert_eq!(first["metrics"][0]["reset_at"], reset_rfc3339);
        assert_eq!(first["sections"][1]["reset_at"], reset_rfc3339);
        assert_eq!(first["metrics"][0]["severity"], "low");
    }

    #[test]
    fn report_reset_metadata_follows_multi_metric_order() {
        let weekly_reset = Utc::now() + chrono::Duration::days(3);
        let window_reset = Utc::now() + chrono::Duration::hours(2);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Kimi(KimiSnapshot {
                plan: Some("Kimi Code".into()),
                weekly_limit: 1_000,
                weekly_used: 200,
                weekly_remaining: 800,
                weekly_reset_at: Some(weekly_reset),
                window_limit: 100,
                window_used: 40,
                window_remaining: 60,
                window_reset_at: Some(window_reset),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Kimi), &state, Utc::now());
        let resets: Vec<_> = projected
            .sections
            .iter()
            .filter_map(|section| match section {
                ReportSection::Metric { reset_at, .. } => Some(*reset_at),
                _ => None,
            })
            .collect();

        // Session-then-weekly, the same order every other vendor projects —
        // Kimi used to come out reversed.
        assert_eq!(resets, vec![Some(window_reset), Some(weekly_reset)]);
    }

    #[test]
    fn json_preserves_non_metric_sections_without_fabricating_percentages() {
        let rendered = render_json_for_primary(
            &[entry(
                "openrouter",
                vec![
                    metric("Credit balance", 25, "$75.00", "$25.00 used"),
                    ReportSection::Spacer,
                    ReportSection::Text {
                        label: "Resets".into(),
                        value: "in 9d".into(),
                    },
                    ReportSection::Block {
                        label: "Usage by period".into(),
                        body: vec!["today $1.00 · week $5.00".into()],
                    },
                ],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["metrics"].as_array().unwrap().len(), 1);
        assert_eq!(first["sections"][1]["type"], "spacer");
        assert_eq!(first["sections"][2]["type"], "text");
        assert!(first["sections"][2].get("percent").is_none());
        assert_eq!(first["sections"][3]["type"], "block");
        assert_eq!(first["sections"][3]["body"][0], "today $1.00 · week $5.00");
    }

    #[test]
    fn real_panel_projection_keeps_openrouter_blocks() {
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Openrouter(OpenRouterSnapshot {
                label: "OR".into(),
                total_credits: 100.0,
                total_usage: 25.0,
                usage_daily: 1.0,
                usage_weekly: 5.0,
                usage_monthly: 25.0,
                is_free_tier: false,
                limit: None,
                limit_remaining: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Openrouter), &state, Utc::now());
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Block { label, .. } if label == "Usage by period"
        )));
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Block { label, .. } if label == "Tier"
        )));
        let text = render_text(&[projected]);
        assert!(text.contains("Usage by period"), "{text}");
        assert!(
            text.contains("today $1.00 · week $5.00 · month $25.00"),
            "{text}"
        );
    }

    #[test]
    fn real_balance_text_is_not_exposed_as_a_percentage_metric() {
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Deepseek(DeepseekSnapshot {
                is_available: true,
                balance: 12.5,
                granted: 2.5,
                topped_up: 10.0,
                currency: "USD".into(),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Deepseek), &state, Utc::now());
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Text { label, value } if label == "Balance" && value == "$12.50"
        )));
        assert!(
            !projected
                .sections
                .iter()
                .any(|section| matches!(section, ReportSection::Metric { .. }))
        );
    }

    #[test]
    fn failed_entries_do_not_duplicate_tui_retry_rows() {
        let failed = entry_from_state(
            &TabId::vendor(VendorId::Openai),
            &TabState::Error("not \x1b[31msigned in\u{202e}".into()),
            Utc::now(),
        );
        assert_eq!(failed.error.as_deref(), Some("not [31msigned in"));
        assert!(failed.sections.is_empty());
    }

    #[test]
    fn exit_is_nonzero_only_when_every_entry_failed() {
        let mut failed = entry("openai", Vec::new());
        failed.error = Some("not signed in".into());
        assert_eq!(report_exit_code(&[failed]), 1);

        let mut failed = entry("openai", Vec::new());
        failed.error = Some("not signed in".into());
        assert_eq!(report_exit_code(&[failed, entry("cursor", Vec::new())]), 0);
    }

    /// The bar hides a tile once ANY window is maxed, even while the other
    /// window still has room — a maxed 5h window gates usage just as hard
    /// as a maxed weekly one.
    #[test]
    fn exhausted_when_any_window_hits_100() {
        use crate::usage::UsageWindow;
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Zai(crate::usage::ZaiSnapshot {
                plan: "GLM Coding Pro".into(),
                session: Some(UsageWindow {
                    utilization_pct: 100,
                    resets_at: Some(Utc::now() + chrono::Duration::hours(1)),
                    window_duration: chrono::Duration::hours(5),
                }),
                weekly: Some(UsageWindow {
                    utilization_pct: 15,
                    resets_at: Some(Utc::now() + chrono::Duration::days(2)),
                    window_duration: chrono::Duration::days(7),
                }),
                mcp: None,
                usage_stats: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Zai), &state, Utc::now());
        assert!(projected.exhausted);

        // The same snapshot at 99% is still very much in the bar.
        let mut recoverable = state;
        let TabState::Ready(ready) = &mut recoverable else {
            unreachable!()
        };
        if let VendorSnapshot::Zai(snap) = &mut ready.snapshot {
            snap.session.as_mut().unwrap().utilization_pct = 99;
        }
        let recovered = entry_from_state(&TabId::vendor(VendorId::Zai), &recoverable, Utc::now());
        assert!(!recovered.exhausted);
    }

    /// Balance vendors: a balance at (or below) zero is spent; a spend row
    /// of $0 is an untouched account and must NOT read as exhausted.
    #[test]
    fn exhausted_when_balance_spent_but_not_when_spend_is_zero() {
        let spent = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Deepseek(DeepseekSnapshot {
                is_available: true,
                balance: 0.0,
                granted: 5.0,
                topped_up: 5.0,
                currency: "USD".into(),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Deepseek), &spent, Utc::now());
        assert!(projected.exhausted);

        assert!(rows_exhausted([
            ("Balance", None, "-$5.71"), // OpenRouter overrun debt
        ]));
        assert!(!rows_exhausted([
            ("Spend this month", None, "$0.00"),
            ("Balance", None, "$12.50"),
        ]));
        assert!(!rows_exhausted([
            ("Session (5h)", Some(42), "42%"),
            ("Weekly (7d)", Some(88), "88%"),
        ]));
    }

    /// Every entry — errors included — carries its vendor's logo asset id so
    /// frontends can brand a tab before the first successful fetch.
    #[test]
    fn every_entry_carries_its_vendor_logo_slug() {
        let failed = TabState::Error("not signed in".into());
        let anthropic = entry_from_state(&TabId::account("gmail"), &failed, Utc::now());
        assert_eq!(anthropic.logo, "claude");
        let cursor = entry_from_state(&TabId::vendor(VendorId::Cursor), &failed, Utc::now());
        assert_eq!(cursor.logo, "cursor");
        let supergrok = entry_from_state(&TabId::vendor(VendorId::Supergrok), &failed, Utc::now());
        assert_eq!(supergrok.logo, "grok", "same brand shares one asset");

        let rendered = render_json_for_primary(&[cursor], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["logo"], "cursor");
        assert_eq!(value["entries"][0]["exhausted"], false);
    }

    /// A vendor's only account drops the " · <suffix>" from display_name —
    /// a lone "Kimi · 1" tab reads as just "Kimi". With a sibling present
    /// (ready, erroring, or exhausted) both keep their suffixes; the machine
    /// `name` is never touched.
    #[test]
    fn solo_accounts_drop_their_display_suffix() {
        fn with_id(mut entry: Entry, id: &str, display: &str) -> Entry {
            entry.id = id.into();
            entry.display_name = display.into();
            entry
        }
        // kimi has ONLY the numbered account → solo. zai has two numbered
        // accounts → siblings. deepseek is a bare default entry.
        let solo = with_id(entry("kimi · 1", Vec::new()), "kimi@1", "Kimi · 1");
        let twin = with_id(entry("zai · 1", Vec::new()), "zai@1", "Z.AI · 1");
        let twin2 = with_id(entry("zai · 2", Vec::new()), "zai@2", "Z.AI · 2");
        let default = with_id(entry("deepseek", Vec::new()), "deepseek", "DeepSeek");

        let mut entries = vec![solo, twin, twin2, default];
        apply_solo_display_names(&mut entries);
        assert_eq!(entries[0].display_name, "Kimi", "solo loses its suffix");
        assert_eq!(entries[0].name, "kimi · 1", "machine name untouched");
        assert_eq!(entries[1].display_name, "Z.AI · 1", "siblings keep theirs");
        assert_eq!(entries[2].display_name, "Z.AI · 2");
        assert_eq!(
            entries[3].display_name, "DeepSeek",
            "default entry unchanged"
        );

        // A vendor with default + one numbered account is NOT solo — the
        // number still distinguishes the account from the default's tile.
        let mut pair = vec![
            with_id(entry("kimi", Vec::new()), "kimi", "Kimi"),
            with_id(entry("kimi · 1", Vec::new()), "kimi@1", "Kimi · 1"),
        ];
        apply_solo_display_names(&mut pair);
        assert_eq!(pair[1].display_name, "Kimi · 1");

        // JSON ships the trimmed name.
        let rendered = render_json_for_primary(&[entries[0].clone()], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["display_name"], "Kimi");
    }

    /// The provider name for a solo entry comes from the entry id's vendor
    /// segment, not from string-splitting display_name: a label that itself
    /// contains the " · " separator (any free-text account label can) must
    /// not confuse the parse. `Kimi · x · y` solo still reads "Kimi".
    #[test]
    fn solo_display_name_survives_a_label_containing_the_separator() {
        let mut solo = entry("kimi · x · y", Vec::new());
        solo.id = "kimi@x · y".into();
        solo.display_name = "Kimi · x · y".into();
        apply_solo_display_names(std::slice::from_mut(&mut solo));
        assert_eq!(
            solo.display_name, "Kimi",
            "solo display must be the provider name from the id, not a split artifact"
        );
    }

    /// U6: report JSON contract — text rows expose window="session"/"weekly"
    /// with ordered resets, and video rows expose window as a JSON `null`
    /// (not a missing key) so a future metric_window regression that
    /// re-classifies a video row would surface immediately.
    #[test]
    fn u6_report_json_carries_minimax_windows_and_video_null() {
        let session_reset = Utc::now() + chrono::Duration::hours(2);
        let weekly_reset = Utc::now() + chrono::Duration::days(3);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Minimax(MinimaxSnapshot {
                plan: "MiniMax Pro".into(),
                session: UsageWindow {
                    utilization_pct: 25,
                    resets_at: Some(session_reset),
                    window_duration: chrono::Duration::hours(5),
                },
                weekly: UsageWindow {
                    utilization_pct: 40,
                    resets_at: Some(weekly_reset),
                    window_duration: chrono::Duration::days(7),
                },
                video_session: Some(UsageWindow {
                    utilization_pct: 30,
                    resets_at: Some(Utc::now() + chrono::Duration::hours(10)),
                    window_duration: chrono::Duration::hours(24),
                }),
                video_weekly: Some(UsageWindow {
                    utilization_pct: 35,
                    resets_at: Some(Utc::now() + chrono::Duration::days(5)),
                    window_duration: chrono::Duration::days(7),
                }),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let entry = entry_from_state(&TabId::vendor(VendorId::Minimax), &state, Utc::now());
        let rendered = render_json_for_primary(&[entry], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let sections = value["entries"][0]["sections"]
            .as_array()
            .expect("sections must be an array");
        // Walk every metric section: text rows expose their windows, video
        // rows expose `null` and carry a label that contains "Video".
        let mut session_seen = false;
        let mut weekly_seen = false;
        let mut video_session_seen = false;
        let mut video_weekly_seen = false;
        for (idx, section) in sections.iter().enumerate() {
            if section["type"] != "metric" {
                continue;
            }
            let label = section["label"].as_str().unwrap_or("");
            match label {
                "Session (5h)" => {
                    assert_eq!(
                        section["window"],
                        serde_json::Value::String("session".into()),
                        "Session (5h) window must be 'session' (idx={idx})"
                    );
                    assert_eq!(
                        section["reset_at"],
                        serde_json::Value::String(
                            session_reset.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
                        ),
                        "Session (5h) reset_at must match input order"
                    );
                    // The window's own clock rides along: with the reset it
                    // gives the countdown figure its time-remaining band.
                    assert_eq!(
                        section["window_seconds"],
                        serde_json::json!(5 * 3600),
                        "Session (5h) window_seconds must be the 5h clock (idx={idx})"
                    );
                    session_seen = true;
                }
                "Weekly" => {
                    assert_eq!(
                        section["window"],
                        serde_json::Value::String("weekly".into()),
                        "Weekly window must be 'weekly' (idx={idx})"
                    );
                    assert_eq!(
                        section["reset_at"],
                        serde_json::Value::String(
                            weekly_reset.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
                        ),
                        "Weekly reset_at must match input order"
                    );
                    assert_eq!(
                        section["window_seconds"],
                        serde_json::json!(7 * 24 * 3600),
                        "Weekly window_seconds must be the 7d clock (idx={idx})"
                    );
                    weekly_seen = true;
                }
                "Video (5h)" => {
                    assert_eq!(
                        section["window"],
                        serde_json::Value::Null,
                        "Video (5h) window must serialize as JSON null, not missing"
                    );
                    assert!(section.get("window").is_some(), "window key present");
                    video_session_seen = true;
                }
                "Video weekly" => {
                    assert_eq!(
                        section["window"],
                        serde_json::Value::Null,
                        "Video weekly window must serialize as JSON null, not missing"
                    );
                    assert!(section.get("window").is_some(), "window key present");
                    video_weekly_seen = true;
                }
                other => panic!("unexpected metric label: {other}"),
            }
        }
        assert!(session_seen && weekly_seen, "text rows must be present");
        assert!(
            video_session_seen && video_weekly_seen,
            "video rows must be present when the snapshot reports them"
        );

        // Order pinned: text rows come before video rows in the array, so
        // a future reorder cannot silently let a video row lead the list.
        let labels: Vec<&str> = sections
            .iter()
            .filter(|s| s["type"] == "metric")
            .map(|s| s["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            vec!["Session (5h)", "Weekly", "Video (5h)", "Video weekly"]
        );

        // The precomputed headline rides these same sections (see
        // `entry_from_state`): the fullest window leads and carries its own
        // reset, so a no-window fallback surface (or an older binary's local
        // walk) can never disagree with the window rows pinned above.
        let headline = &value["entries"][0]["headline"];
        assert_eq!(
            headline["label"], "Weekly",
            "fullest row leads (40% > 25/30/35)"
        );
        assert_eq!(headline["text"], "40%");
        assert_eq!(headline["percent"], 40);
        assert_eq!(
            headline["reset_at"],
            serde_json::Value::String(
                weekly_reset.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
            ),
            "the headline window's reset travels with the pick"
        );
    }
}
