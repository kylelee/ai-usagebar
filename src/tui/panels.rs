//! Native ratatui panels.
//!
//! Each vendor projects its snapshot into a sequence of [`Section`]s — either
//! a metric (gauge + footnote) or a free-form text block. The renderer lays
//! them out vertically with consistent spacing so every panel has the same
//! visual rhythm regardless of vendor.
//!
//! Progress bars use Bubble Tea-style block glyphs that scale to the available
//! width, so on a wide monitor you get long, readable bars instead of the
//! 20-char Pango ones the Waybar tooltip is stuck with.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_bubbletea_components::{Progress, Spinner, SpinnerFrames};
use ratatui_bubbletea_theme::BubbleTheme;

use crate::format::{money, usd};
use crate::pacing::PaceSeverity;
use crate::pango::severity_for;
use crate::theme::Theme;
use crate::tui::app::TabState;
use crate::tui::style::{bubble_theme, color, progress_theme, severity_color};
use crate::usage::VendorSnapshot;

// The neutral section projection lives in `crate::tabs`; these re-exports
// keep every historical `tui::panels` path (widget aggregate, TUI view and
// context overlays, app tests) resolving unchanged.
pub use crate::tabs::Section;
pub(crate) use crate::tabs::{SectionProjection, sections_with_metadata_for};

/// Compact one-line projection of a vendor snapshot for the Overview: a short
/// plan/tier sub-label (may be empty) plus a few key metric cells — a percent
/// or a balance — each carrying a severity for coloring. Same numbers as
/// [`sections_for`], flattened for a dense multi-vendor list. The vendor's name
/// is supplied by the caller, so it is not repeated here.
pub fn compact_cells(snapshot: &VendorSnapshot) -> (String, Vec<(String, PaceSeverity)>) {
    let pct = |label: &str, p: i32| (format!("{label} {p}%"), severity_for(p));
    // Named `*_cell` so neither shadows `format::{usd, money}`, which they
    // wrap — the cell is the string plus the severity the Overview colours it with.
    let usd_cell = |v: f64| (usd(v), PaceSeverity::Low);
    let money_cell = |v: f64, c: &str| (money(v, c), PaceSeverity::Low);
    let (plan, mut cells) = match snapshot {
        VendorSnapshot::Anthropic(s) => {
            let mut cells = vec![
                pct("S", s.session.utilization_pct),
                pct("W", s.weekly.utilization_pct),
            ];
            if let Some(sonnet) = &s.sonnet {
                cells.push(pct("Son", sonnet.utilization_pct));
            }
            (s.plan.clone(), cells)
        }
        VendorSnapshot::AnthropicApi(s) => {
            let cell = match s.pct() {
                Some(p) => pct("spend", p),
                None => (format!("{}/mo", usd(s.spent)), PaceSeverity::Low),
            };
            (String::new(), vec![cell])
        }
        VendorSnapshot::OpenaiApi(s) => {
            let cell = match s.pct() {
                Some(p) => pct("spend", p),
                None => (format!("{}/30d", usd(s.spent)), PaceSeverity::Low),
            };
            (String::new(), vec![cell])
        }
        VendorSnapshot::Openai(s) => {
            let mut cells = Vec::new();
            if let Some(w) = &s.session {
                cells.push(pct("5h", w.utilization_pct));
            }
            if let Some(w) = &s.weekly {
                cells.push(pct("7d", w.utilization_pct));
            }
            if cells.is_empty() {
                cells.push(("—".into(), PaceSeverity::Low));
            }
            (s.plan.clone(), cells)
        }
        VendorSnapshot::Zai(s) => {
            let mut cells = Vec::new();
            if let Some(w) = &s.session {
                cells.push(pct("S", w.utilization_pct));
            }
            if let Some(w) = &s.weekly {
                cells.push(pct("W", w.utilization_pct));
            }
            if cells.is_empty() {
                cells.push(("—".into(), PaceSeverity::Low));
            }
            (s.plan.clone(), cells)
        }
        VendorSnapshot::Openrouter(s) => (String::new(), vec![usd_cell(s.balance())]),
        VendorSnapshot::Deepseek(s) => (String::new(), vec![money_cell(s.balance, &s.currency)]),
        VendorSnapshot::Kimi(s) => (
            s.plan.clone().unwrap_or_default(),
            vec![pct("wk", s.weekly_pct()), pct("5h", s.window_pct())],
        ),
        VendorSnapshot::Kilo(s) => (String::new(), vec![usd_cell(s.balance)]),
        VendorSnapshot::Novita(s) => (String::new(), vec![usd_cell(s.available)]),
        VendorSnapshot::Moonshot(s) => (String::new(), vec![money_cell(s.available, &s.currency)]),
        VendorSnapshot::Grok(s) => (String::new(), vec![usd_cell(s.balance)]),
        VendorSnapshot::SuperGrok(s) => (s.plan.clone(), vec![pct(s.period.short(), s.weekly_pct)]),
        VendorSnapshot::Antigravity(s) => (
            s.plan.clone(),
            vec![
                pct("S", s.session.utilization_pct),
                pct("W", s.weekly.utilization_pct),
            ],
        ),
        VendorSnapshot::Cursor(s) => (
            s.plan.clone(),
            vec![pct("auto", s.auto_pct), pct("premium", s.api_pct)],
        ),
        VendorSnapshot::Minimax(s) => (
            s.plan.clone(),
            vec![
                pct("S", s.session.utilization_pct),
                pct("W", s.weekly.utilization_pct),
            ],
        ),
        VendorSnapshot::Kiro(s) => (s.plan.clone(), vec![pct("credits", s.pct())]),
        VendorSnapshot::OpenCodeGo(s) => {
            let cells = [
                ("rolling", s.rolling.as_ref()),
                ("weekly", s.weekly.as_ref()),
                ("monthly", s.monthly.as_ref()),
            ]
            .into_iter()
            .filter_map(|(label, window)| {
                window.map(|window| pct(label, window.percent.round().clamp(0.0, 100.0) as i32))
            })
            .collect();
            ("OpenCode Go".into(), cells)
        }
    };

    for (text, _) in &mut cells {
        *text = crate::display::sanitize_untrusted_field(text);
    }
    (crate::display::sanitize_untrusted_field(&plan), cells)
}

/// The single most-relevant percentage for a vendor in the Overview — what its
/// per-row mini bar shows. Mirrors the macOS menu bar's headline: Cursor is the
/// combined included-total, quota vendors the most-exhausted window; balance
/// vendors have no meaningful percentage (`None` → no bar).
pub fn headline_pct(snapshot: &VendorSnapshot) -> Option<i32> {
    match snapshot {
        VendorSnapshot::Anthropic(s) => [
            Some(s.session.utilization_pct),
            Some(s.weekly.utilization_pct),
            s.sonnet.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::AnthropicApi(s) => s.pct(),
        VendorSnapshot::OpenaiApi(s) => s.pct(),
        VendorSnapshot::Openai(s) => [
            s.session.as_ref().map(|w| w.utilization_pct),
            s.weekly.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::Zai(s) => [
            s.session.as_ref().map(|w| w.utilization_pct),
            s.weekly.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::Kimi(s) => Some(s.weekly_pct().max(s.window_pct())),
        VendorSnapshot::Antigravity(s) => {
            Some(s.session.utilization_pct.max(s.weekly.utilization_pct))
        }
        VendorSnapshot::Cursor(s) => (!s.unlimited).then_some(s.total_pct),
        VendorSnapshot::Minimax(s) => Some(s.session.utilization_pct.max(s.weekly.utilization_pct)),
        VendorSnapshot::Kiro(s) => Some(s.pct()),
        VendorSnapshot::OpenCodeGo(s) => [
            s.rolling
                .as_ref()
                .map(|window| window.percent.round() as i32),
            s.weekly
                .as_ref()
                .map(|window| window.percent.round() as i32),
            s.monthly
                .as_ref()
                .map(|window| window.percent.round() as i32),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::SuperGrok(s) => Some(s.weekly_pct),
        VendorSnapshot::Openrouter(_)
        | VendorSnapshot::Deepseek(_)
        | VendorSnapshot::Kilo(_)
        | VendorSnapshot::Novita(_)
        | VendorSnapshot::Moonshot(_)
        | VendorSnapshot::Grok(_) => None,
    }
}

/// Build the section list for the currently-active vendor's snapshot.
pub fn sections_for(tab: &TabState, now: DateTime<Utc>, pace_tolerance: u32) -> Vec<Section> {
    sections_with_metadata_for(tab, now, pace_tolerance)
        .into_iter()
        .map(|projected| projected.section)
        .collect()
}

/// Render the given sections into `area`. Lays them out vertically; metric
/// rows take 2 lines (label+gauge / footnote), text and spacer rows take 1.
///
/// The trailing "Updated …" footer is detected (the last `Text` section)
/// and pinned to the bottom of the area, with the slack absorbed *between*
/// content and footer. This way shorter vendor panels (OpenRouter, Z.AI)
/// don't leave a giant gap below the footer.
pub fn render(f: &mut Frame, area: Rect, theme: &Theme, sections: &[Section]) {
    if sections.is_empty() {
        return;
    }
    let bubble = bubble_theme(theme);
    // Heuristic: if the last section is a Text starting with "  Updated",
    // pin it to the bottom. Otherwise just lay everything out top-down.
    let pin_last =
        matches!(sections.last(), Some(Section::Text { value, .. }) if value.contains("Updated"));

    let body_end = if pin_last {
        sections.len() - 1
    } else {
        sections.len()
    };
    let mut constraints: Vec<Constraint> =
        sections[..body_end].iter().map(section_height).collect();

    if pin_last {
        constraints.push(Constraint::Min(0)); // slack between body and footer
        constraints.push(section_height(sections.last().unwrap()));
    } else {
        constraints.push(Constraint::Min(0));
    }

    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (i, s) in sections[..body_end].iter().enumerate() {
        render_section(f, chunks[i], theme, &bubble, s);
    }
    if pin_last {
        render_section(
            f,
            chunks[chunks.len() - 1],
            theme,
            &bubble,
            sections.last().unwrap(),
        );
    }
}

fn section_height(s: &Section) -> Constraint {
    match s {
        Section::Title { .. } => Constraint::Length(2),
        Section::Metric { .. } => Constraint::Length(3),
        Section::Text { .. } => Constraint::Length(1),
        Section::Block { body, .. } => Constraint::Length(1 + body.len() as u16),
        Section::Spacer => Constraint::Length(1),
    }
}

fn render_section(f: &mut Frame, area: Rect, theme: &Theme, bubble: &BubbleTheme, s: &Section) {
    match s {
        Section::Title { left, right } => {
            // Left: bold accent-colored plan/vendor label. Right: dim-styled
            // "Updated HH:MM:SS" pinned to the right edge of the title row.
            let left_line = Line::from(Span::styled(
                format!("  {} {left}", bubble.symbols.selected),
                bubble.title,
            ));
            f.render_widget(Paragraph::new(left_line), area);
            if let Some(rt) = right {
                let right_line =
                    Line::from(Span::styled(format!("{rt}  "), bubble.muted)).right_aligned();
                f.render_widget(Paragraph::new(right_line), area);
            }
        }
        Section::Metric {
            label,
            pct,
            severity,
            value_label,
            footnote,
        } => render_metric(
            f,
            area,
            theme,
            bubble,
            label,
            *pct,
            *severity,
            value_label,
            footnote,
        ),
        Section::Text { label, value } => {
            if label.is_empty() && value.contains("Loading") {
                render_loading(f, area, bubble);
                return;
            }
            if label == "Error" {
                let line = Line::from(vec![
                    bubble.error(format!("  {} ", bubble.symbols.cross)),
                    Span::styled(value.clone(), bubble.error.add_modifier(Modifier::BOLD)),
                ]);
                f.render_widget(Paragraph::new(line), area);
                return;
            }
            let mut spans = Vec::new();
            if !label.is_empty() {
                spans.push(Span::styled(
                    format!("  {label}  "),
                    bubble.text.add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(value.clone(), bubble.muted));
            f.render_widget(Paragraph::new(Line::from(spans)), area);
        }
        Section::Block { label, body } => render_block(f, area, bubble, label, body),
        Section::Spacer => {}
    }
}

fn render_loading(f: &mut Frame, area: Rect, bubble: &BubbleTheme) {
    let frames = SpinnerFrames::DOTS;
    let frame_count = frames.frames().len().max(1);
    let frame = chrono::Utc::now().timestamp_millis().unsigned_abs() as usize / 120;
    let mut spinner = Spinner::new()
        .frames(frames)
        .label("Fetching usage data")
        .theme(*bubble);
    for _ in 0..(frame % frame_count) {
        spinner.tick();
    }
    f.render_widget(&spinner, area);
}

#[allow(clippy::too_many_arguments)]
fn render_metric(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    bubble: &BubbleTheme,
    label: &str,
    pct: u16,
    severity: PaceSeverity,
    value_label: &str,
    footnote: &str,
) {
    let bar_color = severity_color(theme, bubble, severity);
    let bar_empty = color(&theme.bar_empty).unwrap_or(bubble.palette.selected_background);

    let inner = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Row 1: label
    let label_line = Line::from(Span::styled(
        format!("  {label}"),
        bubble.text.add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(label_line), inner[0]);

    // Row 2: gauge spanning most of the width + value annotation on the right
    let row = inner[1];
    let value_w = value_label.chars().count() as u16 + 2;
    let gauge_area = Rect {
        x: row.x + 2,
        y: row.y,
        width: row.width.saturating_sub(value_w + 4),
        height: 1,
    };
    let value_area = Rect {
        x: gauge_area.x + gauge_area.width + 1,
        y: row.y,
        width: value_w,
        height: 1,
    };
    let progress_theme = progress_theme(*bubble, bar_color, bar_empty);
    let progress = Progress::from_percent(pct)
        .theme(progress_theme)
        .show_percentage(false);
    f.render_widget(&progress, gauge_area);
    let value = Paragraph::new(Line::from(Span::styled(
        value_label.to_string(),
        Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
    )));
    f.render_widget(value, value_area);

    // Row 3: footnote (dim)
    let foot = Line::from(Span::styled(format!("    {footnote}"), bubble.muted));
    f.render_widget(Paragraph::new(foot), inner[2]);
}

fn render_block(f: &mut Frame, area: Rect, bubble: &BubbleTheme, label: &str, body: &[String]) {
    let mut lines = vec![Line::from(Span::styled(
        format!("  {label}"),
        bubble.text.add_modifier(Modifier::BOLD),
    ))];
    for b in body {
        lines.push(Line::from(Span::styled(format!("    {b}"), bubble.muted)));
    }
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::warning_label;
    use crate::usage::{
        AnthropicSnapshot, Cents, ExtraUsage, KimiSnapshot, OpenAiCredits, OpenAiSnapshot,
        OpenAiSource, OpenRouterSnapshot, UsageWindow, ZaiSnapshot,
    };
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap()
    }

    fn ready(snapshot: VendorSnapshot) -> TabState {
        TabState::Ready(Box::new(crate::tui::app::ReadyTab {
            snapshot,
            stale: false,
            last_error: None,
            fetched_at: Some(now() - chrono::Duration::seconds(15)),
        }))
    }

    #[test]
    fn anthropic_sections_include_all_three_windows_when_present() {
        let snap = AnthropicSnapshot {
            plan: "Max 20x".into(),
            session: UsageWindow {
                utilization_pct: 60,
                resets_at: Some(now() + chrono::Duration::hours(1)),
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 30,
                resets_at: Some(now() + chrono::Duration::days(3)),
                window_duration: chrono::Duration::days(7),
            },
            sonnet: Some(UsageWindow {
                utilization_pct: 5,
                resets_at: Some(now() + chrono::Duration::hours(2)),
                window_duration: chrono::Duration::days(7),
            }),
            scoped: vec![],
            extra: Some(ExtraUsage {
                limit: Some(Cents(5000)),
                spent: Cents(250),
                currency: None,
                decimal_places: Some(2),
            }),
        };
        let sections = sections_for(&ready(VendorSnapshot::Anthropic(snap)), now(), 5);
        // Title (carries "Updated …" inline now) + 4 metrics (3 windows +
        // extra) each preceded by a Spacer. 1 + 4*2 = 9 sections.
        assert_eq!(sections.len(), 9);
        assert!(matches!(sections[0], Section::Title { .. }));
        // Title's right-aligned slot should carry the timestamp.
        if let Section::Title { right, .. } = &sections[0] {
            assert!(right.as_deref().is_some_and(|r| r.starts_with("Updated ")));
        } else {
            panic!("expected first section to be Title");
        }
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 4);
    }

    #[test]
    fn anthropic_uncapped_extra_shows_spend_without_a_denominator() {
        // The #30 shape: `monthly_limit: null` (Pro). The panel must show the
        // spend alone — not "of $0.00", not an invented percentage.
        let snap = AnthropicSnapshot {
            plan: "Pro".into(),
            session: UsageWindow {
                utilization_pct: 10,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 20,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            },
            sonnet: None,
            scoped: vec![],
            extra: Some(ExtraUsage {
                limit: None,
                spent: Cents(14157),
                currency: Some("BRL".into()),
                decimal_places: Some(2),
            }),
        };
        let sections = sections_for(&ready(VendorSnapshot::Anthropic(snap)), now(), 5);
        let extra = sections
            .iter()
            .find_map(|s| match s {
                Section::Metric {
                    label,
                    pct,
                    value_label,
                    footnote,
                    ..
                } if label == "Extra usage" => Some((*pct, value_label.clone(), footnote.clone())),
                _ => None,
            })
            .expect("uncapped extra usage must still render a section");
        assert_eq!(extra.0, 0);
        // Non-vacuous currency pin: fmt_dollars would say "$141.57" here.
        assert_eq!(extra.1, "R$141.57");
        assert!(
            !extra.1.contains(" of "),
            "no denominator to show: {}",
            extra.1
        );
        assert_eq!(extra.2, "no monthly limit reported");
    }

    #[test]
    fn anthropic_omits_sonnet_and_extra_when_absent() {
        let snap = AnthropicSnapshot {
            plan: "Pro".into(),
            session: UsageWindow {
                utilization_pct: 10,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 5,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            },
            sonnet: None,
            scoped: vec![],
            extra: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Anthropic(snap)), now(), 5);
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 2);
    }

    #[test]
    fn openrouter_always_has_balance_metric_and_period_block() {
        let snap = OpenRouterSnapshot {
            label: "OR".into(),
            total_credits: 100.0,
            total_usage: 25.0,
            usage_daily: 1.0,
            usage_weekly: 5.0,
            usage_monthly: 25.0,
            is_free_tier: false,
            limit: None,
            limit_remaining: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openrouter(snap)), now(), 5);
        assert!(matches!(sections[0], Section::Title { .. }));
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Metric { label, .. } if label == "Credit balance"))
        );
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Block { label, .. } if label == "Usage by period"))
        );
    }

    /// #118 reached every frontend, not just Waybar: the panel row is what the
    /// Omarchy, GNOME and KDE plugins colour and label from, so the debt has to
    /// survive the projection with its sign and its severity intact.
    #[test]
    fn openrouter_debt_reaches_the_panel_row_red_and_signed() {
        let snap = OpenRouterSnapshot {
            label: "OR".into(),
            total_credits: 0.0,
            total_usage: 5.71,
            usage_daily: 1.0,
            usage_weekly: 5.0,
            usage_monthly: 5.71,
            is_free_tier: false,
            limit: None,
            limit_remaining: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openrouter(snap.clone())), now(), 5);
        let metric = sections
            .iter()
            .find_map(|s| match s {
                Section::Metric {
                    label,
                    value_label,
                    severity,
                    footnote,
                    ..
                } if label == "Credit balance" => Some((value_label, severity, footnote)),
                _ => None,
            })
            .expect("no credit balance metric");
        assert_eq!(metric.0, "-$5.71");
        assert_eq!(*metric.1, PaceSeverity::Critical);
        assert_eq!(metric.2, "$5.71 of $0.00 used (0%)");

        // ...and the same number in the dense Overview list.
        let (_, cells) = compact_cells(&VendorSnapshot::Openrouter(snap));
        assert_eq!(cells[0].0, "-$5.71");
    }

    #[test]
    fn zai_no_windows_renders_message() {
        let snap = ZaiSnapshot {
            plan: "GLM".into(),
            session: None,
            weekly: None,
            mcp: None,
            usage_stats: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Zai(snap)), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("no usage windows reported")
        )));
    }

    /// A usage-only (pay-as-you-go) key reports the 7-day aggregate instead
    /// of the "no usage windows" message.
    #[test]
    fn zai_usage_only_renders_seven_day_stats() {
        let snap = ZaiSnapshot {
            plan: "GLM Coding Pay-as-you-go".into(),
            session: None,
            weekly: None,
            mcp: None,
            usage_stats: Some(crate::usage::ZaiUsageStats {
                prompts: 1234,
                tokens: 5_678_901,
            }),
        };
        let sections = sections_for(&ready(VendorSnapshot::Zai(snap)), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { label, value, .. }
                if label == "7-day usage" && value.contains("1.2k prompts") && value.contains("5.7M tokens")
        )));
        assert!(!sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("no usage windows reported")
        )));
    }

    #[test]
    fn openai_no_windows_renders_message() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: None,
            weekly: None,
            code_review: None,
            credits: None,
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("no usage windows reported")
        )));
    }

    #[test]
    fn loading_state_yields_loading_section() {
        let sections = sections_for(&TabState::Loading, now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("Loading")
        )));
    }

    #[test]
    fn error_state_includes_retry_hint() {
        let sections = sections_for(&TabState::Error("token expired".into()), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("token expired")
        )));
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("`r` to retry")
        )));
    }

    #[test]
    fn error_state_advertises_the_settings_overlay() {
        let sections = sections_for(&TabState::Error("token expired".into()), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("`s` to configure")
        )));
    }

    #[test]
    fn openai_with_credits_renders_block() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: Some(UsageWindow {
                utilization_pct: 1,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            }),
            weekly: Some(UsageWindow {
                utilization_pct: 0,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            }),
            code_review: None,
            credits: Some(OpenAiCredits {
                balance: "$5.00".into(),
                has_credits: true,
                unlimited: false,
                approx_local_messages: Some((100, 200)),
                approx_cloud_messages: Some((30, 50)),
            }),
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Block { label, .. } if label == "Credits"))
        );
    }

    #[test]
    fn openai_weekly_only_omits_session_section() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Prolite".into(),
            session: None,
            weekly: Some(UsageWindow {
                utilization_pct: 66,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            }),
            code_review: None,
            credits: None,
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(sections.iter().any(|section| matches!(
            section,
            Section::Metric { label, .. } if label == "Codex weekly"
        )));
        assert!(!sections.iter().any(|section| matches!(
            section,
            Section::Metric { label, .. } if label == "Codex 5h"
        )));
    }

    #[test]
    fn kimi_sections_include_weekly_and_window_with_used_over_limit() {
        let now = now();
        let snap = KimiSnapshot {
            plan: Some("LEVEL_INTERMEDIATE".into()),
            weekly_limit: 100,
            weekly_used: 26,
            weekly_remaining: 74,
            weekly_reset_at: Some(now + chrono::Duration::days(4)),
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(now + chrono::Duration::hours(2)),
        };
        let sections = sections_for(&ready(VendorSnapshot::Kimi(snap)), now, 5);
        let metrics: Vec<_> = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .collect();
        assert_eq!(metrics.len(), 2);
        // Session-then-weekly, like every other vendor — Kimi used to come
        // out reversed (the reported ui bug).
        assert!(matches!(
            metrics[0],
            Section::Metric { label, .. } if label == "Rolling window (5h)"
        ));
        assert!(matches!(
            metrics[1],
            Section::Metric { label, .. } if label == "Weekly quota"
        ));

        let find_footnote = |label: &str| -> (String, String) {
            sections
                .iter()
                .find_map(|s| match s {
                    Section::Metric {
                        label: l,
                        value_label,
                        footnote,
                        ..
                    } if l == label => Some((value_label.clone(), footnote.clone())),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing metric {label}"))
        };

        let (weekly_value, weekly_footnote) = find_footnote("Weekly quota");
        assert_eq!(weekly_value, "26%");
        assert!(weekly_footnote.contains("26 / 100"));
        assert!(weekly_footnote.contains("74 remaining"));
        assert!(
            weekly_footnote.contains("4d 0h"),
            "weekly reset countdown: {weekly_footnote}"
        );
        assert!(!weekly_footnote.contains("2026-05-27T")); // not a raw RFC3339

        let (window_value, window_footnote) = find_footnote("Rolling window (5h)");
        assert_eq!(window_value, "15%");
        assert!(window_footnote.contains("15 / 100"));
        assert!(window_footnote.contains("85 remaining"));
        assert!(
            window_footnote.contains("2h 00m"),
            "window reset countdown: {window_footnote}"
        );
        assert!(!window_footnote.contains("2026-05-23T14")); // not a raw RFC3339
    }

    #[test]
    fn kimi_sections_omit_window_when_limit_zero() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: 100,
            weekly_used: 10,
            weekly_remaining: 90,
            weekly_reset_at: None,
            window_limit: 0,
            window_used: 0,
            window_remaining: 0,
            window_reset_at: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Kimi(snap)), now(), 5);
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 1);
    }

    fn cursor_snap() -> crate::usage::CursorSnapshot {
        crate::usage::CursorSnapshot {
            plan: "Ultra".into(),
            auto_pct: 98,
            api_pct: 100,
            total_pct: 99,
            unlimited: false,
            on_demand_enabled: false,
            reset_at: Some(now() + chrono::Duration::days(9)),
        }
    }

    #[test]
    fn compact_cells_flatten_key_metrics_for_the_overview() {
        // Percent vendor (Cursor): plan + two colored pool cells.
        let (plan, cells) = compact_cells(&VendorSnapshot::Cursor(cursor_snap()));
        assert_eq!(plan, "Ultra");
        assert_eq!(cells[0].0, "auto 98%");
        assert_eq!(cells[1].0, "premium 100%");
        assert_eq!(cells[1].1, PaceSeverity::Critical); // 100% is critical

        // Balance vendor (Kilo): no plan, a single money cell, calm severity.
        let (plan, cells) = compact_cells(&VendorSnapshot::Kilo(crate::usage::KiloSnapshot {
            label: "Kilo".into(),
            balance: 8.42,
        }));
        assert!(plan.is_empty());
        assert_eq!(cells, vec![("$8.42".to_string(), PaceSeverity::Low)]);
    }

    #[test]
    fn terminal_controls_are_removed_from_detail_and_overview_fields() {
        let error = TabState::Error("bad\x1b]52;c;Y2FuYXJ5\x07 value".into());
        let sections = sections_for(&error, now(), 5);
        assert!(matches!(
            &sections[1],
            Section::Text { value, .. }
                if value == "bad]52;c;Y2FuYXJ5 value"
                    && !value.chars().any(|ch| ch.is_control())
        ));

        let mut snapshot = cursor_snap();
        snapshot.plan = "Ultra\x1b[2J\x07".into();
        let (plan, _) = compact_cells(&VendorSnapshot::Cursor(snapshot));
        assert_eq!(plan, "Ultra[2J");
        assert!(!plan.chars().any(char::is_control));
    }

    #[test]
    fn headline_pct_is_the_worst_window_or_combined_total() {
        // Cursor: the combined total, not the worse pool (mirrors the menu bar).
        assert_eq!(
            headline_pct(&VendorSnapshot::Cursor(cursor_snap())),
            Some(99)
        );

        // Balance-only vendors have no meaningful percentage → no bar.
        let kilo = VendorSnapshot::Kilo(crate::usage::KiloSnapshot {
            label: "Kilo".into(),
            balance: 8.42,
        });
        assert_eq!(headline_pct(&kilo), None);
    }

    #[test]
    fn cursor_sections_show_both_pools_and_reset() {
        let sections = sections_for(&ready(VendorSnapshot::Cursor(cursor_snap())), now(), 5);
        let metrics: Vec<_> = sections
            .iter()
            .filter_map(|s| match s {
                Section::Metric {
                    label, value_label, ..
                } => Some((label.clone(), value_label.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(metrics.len(), 2, "two pools");
        assert!(
            metrics
                .iter()
                .any(|(l, v)| l == "Cursor Models" && v == "98%")
        );
        assert!(
            metrics
                .iter()
                .any(|(l, v)| l == "Other Models" && v == "100%")
        );
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { label, value } if label == "Resets" && value.contains("9d")
        )));
    }

    #[test]
    fn cursor_unlimited_plan_shows_no_pool_bars() {
        let mut snap = cursor_snap();
        snap.unlimited = true;
        let sections = sections_for(&ready(VendorSnapshot::Cursor(snap)), now(), 5);
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 0);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("Unlimited")
        )));
    }

    fn kiro_snap() -> crate::usage::KiroSnapshot {
        crate::usage::KiroSnapshot {
            plan: "KIRO POWER".into(),
            used: 9943.38,
            limit: 10000.0,
            reset_at: Some(now() + chrono::Duration::days(1)),
        }
    }

    #[test]
    fn kiro_compact_cell_shows_the_credit_percentage() {
        let (plan, cells) = compact_cells(&VendorSnapshot::Kiro(kiro_snap()));
        assert_eq!(plan, "KIRO POWER");
        assert_eq!(
            cells,
            vec![("credits 99%".to_string(), PaceSeverity::Critical)]
        );
    }

    #[test]
    fn kiro_headline_pct_is_the_credit_percentage() {
        assert_eq!(headline_pct(&VendorSnapshot::Kiro(kiro_snap())), Some(99));
    }

    #[test]
    fn kiro_sections_show_the_credit_metric_and_reset() {
        let sections = sections_for(&ready(VendorSnapshot::Kiro(kiro_snap())), now(), 5);
        let metrics: Vec<_> = sections
            .iter()
            .filter_map(|s| match s {
                Section::Metric {
                    label, value_label, ..
                } => Some((label.clone(), value_label.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(metrics, vec![("Credits".to_string(), "99%".to_string())]);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { label, value } if label == "Resets" && value.contains("1d")
        )));
    }

    #[test]
    fn schema_drift_and_generic_code_zero_diagnostics_are_visible_without_http_labels() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: 100,
            weekly_used: 10,
            weekly_remaining: 90,
            weekly_reset_at: None,
            window_limit: 0,
            window_used: 0,
            window_remaining: 0,
            window_reset_at: None,
        };
        let mut schema = ready(VendorSnapshot::Kimi(snap.clone()));
        let TabState::Ready(tab) = &mut schema else {
            unreachable!()
        };
        tab.last_error = Some((0, crate::kimi::fetch::SCHEMA_DRIFT_MESSAGE.into()));
        let schema_sections = sections_for(&schema, now(), 5);
        assert!(schema_sections.iter().any(|section| matches!(
            section,
            Section::Text { label, value } if label == "Kimi API schema drift" && value.is_empty()
        )));

        let mut generic = ready(VendorSnapshot::Kimi(snap));
        let TabState::Ready(tab) = &mut generic else {
            unreachable!()
        };
        tab.last_error = Some((0, "cache lock unavailable".into()));
        let generic_sections = sections_for(&generic, now(), 5);
        assert!(generic_sections.iter().any(|section| matches!(
            section,
            Section::Text { label, value } if label == "Warning" && value == "cache lock unavailable"
        )));
        assert!(!generic_sections.iter().any(|section| matches!(
            section,
            Section::Text { label, .. } if label.starts_with("HTTP")
        )));

        let http = warning_label(
            &VendorSnapshot::Kimi(KimiSnapshot {
                plan: None,
                weekly_limit: 0,
                weekly_used: 0,
                weekly_remaining: 0,
                weekly_reset_at: None,
                window_limit: 0,
                window_used: 0,
                window_remaining: 0,
                window_reset_at: None,
            }),
            &Some((503, "service unavailable".into())),
        );
        assert_eq!(
            http,
            Some(("HTTP 503".into(), "service unavailable".into()))
        );
    }
}
