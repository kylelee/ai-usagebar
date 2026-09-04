//! OpenAI Admin API renderer — trailing-30-day spend, optionally against a
//! configured monthly limit (there is no balance endpoint; the limit is a
//! config value, exactly like the Anthropic Admin vendor).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, usd};
use crate::pacing::PaceSeverity;
use crate::pango::{escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::OpenAiApiSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome, render_shell};
use crate::waybar::WaybarOutput;

impl From<super::fetch::FetchOutcome> for VendorOutcome {
    fn from(o: super::fetch::FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::OpenaiApi(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

pub const DEFAULT_FORMAT: &str = "{oai_headline}";

/// Bar headline: spend-vs-limit with a % when a limit is configured,
/// otherwise the trailing-30-day spend.
fn headline(snap: &OpenAiApiSnapshot) -> String {
    match snap.limit {
        Some(l) if l > 0.0 => format!(
            "{} / ${:.0} · {}%",
            usd(snap.spent),
            l,
            snap.pct().unwrap_or(0)
        ),
        _ => format!("{}/30d", usd(snap.spent)),
    }
}

pub fn build_placeholders(snap: &OpenAiApiSnapshot) -> HashMap<&'static str, String> {
    let pct = snap.pct().unwrap_or(0);
    placeholders(vec![
        ("icon", "󰢗".to_string()),
        ("vendor_short", VendorId::OpenaiApi.short_name().to_string()),
        // Cross-vendor aliases — spend% maps to the session/weekly slots.
        ("session_pct", pct.to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", pct.to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", "OpenAI API".to_string()),
        ("oai_headline", headline(snap)),
        ("oai_spent", usd(snap.spent)),
        (
            "oai_limit",
            snap.limit
                .map(|l| format!("${l:.0}"))
                .unwrap_or_else(|| "—".into()),
        ),
        ("oai_pct", pct.to_string()),
    ])
}

/// Severity keys on the spend-vs-limit %. With no limit there's no signal, so
/// it stays calm (low).
pub fn severity(snap: &OpenAiApiSnapshot) -> PaceSeverity {
    match snap.pct() {
        Some(p) => severity_for(p.min(100)),
        None => PaceSeverity::Low,
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &OpenAiApiSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    render_shell(
        DEFAULT_FORMAT,
        &build_placeholders(snap),
        opts,
        outcome.stale,
        Some(severity(snap)),
        theme,
        render_tooltip(outcome, snap, theme, now),
    )
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &OpenAiApiSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>OpenAI API</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰉹  Spend, last 30 days</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{spent}</span>",
        spent = escape(&usd(snap.spent))
    )));
    match snap.limit {
        Some(l) if l > 0.0 => {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     of ${l:.0} limit ({pct}%)</span>",
                pct = snap.pct().unwrap_or(0)
            )));
        }
        _ => {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     no monthly limit set (add `monthly_limit` under [openai_api])</span>"
            )));
        }
    }

    if !snap.top_items.is_empty() {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  󰋎  Top line items</span>"
        )));
        for (name, value) in snap.top_items.iter().take(5) {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     {} — {}</span>",
                escape(name),
                escape(&usd(*value))
            )));
        }
    }

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰋼  spend consumed, not balance —</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     the Admin API has no balance endpoint</span>"
    )));

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        let (icon, ecolor, header) = if *code == 0 {
            ("󰀪", theme.orange.as_str(), "Sync error".to_string())
        } else if *code >= 500 {
            ("󰅚", theme.red.as_str(), format!("HTTP {code}"))
        } else {
            (
                "󰀪",
                theme.red.as_str(),
                format!(
                    "HTTP {code} — Admin key required (organization settings); a project sk- key is rejected here"
                ),
            )
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>{icon}  {header}</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>     {}</span>",
            escape(msg)
        )));
    }

    let _ = now;
    render_bordered(&lines, theme)
}

// allow: SIZE_OK — one renderer plus its co-located render tests, the
// established vendor-module shape (see anthropic_api/vendor.rs).
#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(
        spent: f64,
        limit: Option<f64>,
        top_items: Vec<(String, f64)>,
        last_error: Option<(u16, String)>,
    ) -> (OpenAiApiSnapshot, VendorOutcome) {
        let snap = OpenAiApiSnapshot {
            spent,
            by_day: Vec::new(),
            top_items,
            limit,
        };
        let o = VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::OpenaiApi(snap.clone()),
            stale: false,
            last_error,
            cache_age: Some(std::time::Duration::from_secs(10)),
        };
        (snap, o)
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

    #[test]
    fn headline_with_limit_shows_spend_limit_and_pct() {
        let (snap, o) = outcome(4.2, Some(50.0), Vec::new(), None);
        let out = render(&o, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$4.20 / $50 · 8%"), "{}", out.text);
        assert!(out.tooltip.contains("of $50 limit (8%)"), "{}", out.tooltip);
    }

    #[test]
    fn headline_without_limit_shows_trailing_spend() {
        let (snap, o) = outcome(4.2, None, Vec::new(), None);
        let out = render(&o, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$4.20/30d"), "{}", out.text);
        assert!(
            out.tooltip.contains("no monthly limit set"),
            "{}",
            out.tooltip
        );
    }

    #[test]
    fn severity_scales_with_spend_pct() {
        assert_eq!(
            severity(&OpenAiApiSnapshot {
                spent: 45.0,
                by_day: Vec::new(),
                top_items: Vec::new(),
                limit: Some(50.0)
            }),
            PaceSeverity::Critical
        );
        assert_eq!(
            severity(&OpenAiApiSnapshot {
                spent: 5.0,
                by_day: Vec::new(),
                top_items: Vec::new(),
                limit: Some(1000.0)
            }),
            PaceSeverity::Low
        );
        assert_eq!(
            severity(&OpenAiApiSnapshot {
                spent: 500.0,
                by_day: Vec::new(),
                top_items: Vec::new(),
                limit: None
            }),
            PaceSeverity::Low
        );
    }

    #[test]
    fn tooltip_lists_top_line_items_with_amounts() {
        let (snap, o) = outcome(
            4.2,
            None,
            vec![("gpt-5.2".into(), 3.5), ("fine-tune job".into(), 0.7)],
            None,
        );
        let out = render(&o, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(
            out.tooltip.contains("gpt-5.2 — $3.50")
                && out.tooltip.contains("fine-tune job — $0.70"),
            "{}",
            out.tooltip
        );
    }

    #[test]
    fn http_401_error_explains_the_admin_key_requirement() {
        let (snap, o) = outcome(4.2, None, Vec::new(), Some((401, "bad key".into())));
        let out = render(&o, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(
            out.tooltip.contains("HTTP 401 — Admin key required"),
            "{}",
            out.tooltip
        );
        assert!(out.tooltip.contains("bad key"), "{}", out.tooltip);
    }

    #[test]
    fn format_overrides_substitute_the_oai_placeholders() {
        let (snap, o) = outcome(4.2, Some(50.0), Vec::new(), None);
        let mut o2 = opts();
        o2.format = Some("{vendor_short} {oai_spent} of {oai_limit} ({oai_pct}%)".into());
        let out = render(&o, &snap, &Theme::default(), &o2, Utc::now());
        assert!(
            out.text.ends_with(">oai $4.20 of $50 (8%)</span>"),
            "{}",
            out.text
        );
    }
}
