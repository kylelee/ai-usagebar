//! TUI app state — vendors, tab selection, per-vendor snapshot cache.

use std::collections::HashSet;

pub use crate::tabs::{
    ANTHROPIC_REFRESH_STAGGER, REFRESH_INTERVAL, ReadyTab, TabId, TabState, refresh_one,
    refresh_stagger, tabs_from_config, tabs_with_desktop,
};
use crate::theme::Theme;
use crate::vendor::VendorId;

#[derive(Debug)]
pub struct App {
    pub tabs_meta: Vec<TabId>,
    pub active: usize,
    pub tabs: Vec<TabState>,
    /// Tab identities with a request currently in flight. Kept separate from
    /// `tabs` so a successful snapshot remains visible while it is refreshed.
    refreshing_tabs: HashSet<TabId>,
    /// Monotonically increasing identity for a complete tab-set replacement.
    /// Background fetches carry this with their tab identity so results from a
    /// previous Settings reload cannot land in a new tab at the old index.
    pub tab_generation: u64,
    /// When `true`, the Overview pane is selected (the virtual first tab that
    /// summarizes every vendor at once) instead of a per-vendor detail tab.
    pub overview: bool,
    /// Which vendors the Overview lists (`[ui] overview_vendors`); `None` = all.
    pub overview_vendors: Option<Vec<VendorId>>,
    pub theme: Theme,
    pub quit: bool,
    /// When `Some`, the Settings overlay is open and consuming key events.
    pub settings: Option<crate::tui::settings::SettingsState>,
    /// Local context monitoring is separately opt-in and never changes the
    /// vendor tab set.
    pub context_enabled: bool,
    /// Monotonic across overlay close/reopen cycles so an old detached scan
    /// can never share the new overlay's first generation number.
    pub context_generation: u64,
    /// When `Some`, the local Claude Code context overlay owns keyboard input.
    pub context: Option<crate::tui::context::ContextState>,
    /// Presentation style for the vendor navigation box (`[ui] vendor_box`).
    pub vendor_box: crate::config::VendorBoxStyle,
    /// Last config hot-reload failure, drawn in the footer until the next
    /// successful reload. Owned by the binary's reload path; app.rs only
    /// stores it.
    pub config_reload_error: Option<String>,
}

impl App {
    pub fn new(tabs_meta: Vec<TabId>) -> Self {
        // Production: resolve the palette from the environment (Omarchy theme
        // if present, else One Dark).
        Self::with_theme(tabs_meta, Theme::default().merged_with_omarchy())
    }

    /// Like [`App::new`] but with an explicit theme. Lets tests build an `App`
    /// without reading the real Omarchy theme file
    /// (`$HOME/.config/omarchy/current/theme/colors.toml`) — `new` resolves
    /// that path and the `$HOME` env var via `merged_with_omarchy`, which is
    /// not hermetic. Production code uses `new`/`new_with_primary`.
    pub fn with_theme(tabs_meta: Vec<TabId>, theme: Theme) -> Self {
        let n = tabs_meta.len();
        Self {
            tabs_meta,
            active: 0,
            tabs: vec![TabState::Loading; n],
            refreshing_tabs: HashSet::new(),
            tab_generation: 0,
            overview: false,
            overview_vendors: None,
            theme,
            quit: false,
            settings: None,
            context_enabled: false,
            context_generation: 0,
            context: None,
            vendor_box: crate::config::VendorBoxStyle::Sidebar,
            config_reload_error: None,
        }
    }

    /// Construct with an initial active tab — usually `[ui] primary` from
    /// config. Silently falls through to index 0 if the requested vendor
    /// isn't present (e.g. it was disabled).
    pub fn new_with_primary(tabs_meta: Vec<TabId>, primary: Option<VendorId>) -> Self {
        let mut app = Self::new(tabs_meta);
        // Default landing is the Overview (show everything at once). An explicit
        // `[ui] primary` opts into opening on that vendor's tab instead.
        if primary.is_some() {
            app.select_primary(primary);
        } else {
            app.overview = true;
        }
        app
    }

    pub fn active_tab_id(&self) -> Option<&TabId> {
        self.tabs_meta.get(self.active)
    }

    pub fn active_vendor(&self) -> Option<VendorId> {
        self.tabs_meta.get(self.active).map(|t| t.vendor)
    }

    /// Replace the tab set — used after a Settings save reloads config, so
    /// tabs added or removed in `config.toml` while the TUI is open (e.g. a
    /// new `[[anthropic.accounts]]` entry) appear without a restart. Every
    /// tab resets to `Loading` (the caller re-spawns fetches). The selected tab
    /// is preserved by identity when possible; otherwise its old position is
    /// clamped in case the list shrank.
    pub fn set_tabs(&mut self, tabs_meta: Vec<TabId>) {
        let selected = self.active_tab_id().cloned();
        let fallback = self.active.min(tabs_meta.len().saturating_sub(1));
        self.tab_generation = self.tab_generation.wrapping_add(1);
        self.active = selected
            .as_ref()
            .and_then(|tab| tabs_meta.iter().position(|candidate| candidate == tab))
            .unwrap_or(fallback);
        self.tabs = vec![TabState::Loading; tabs_meta.len()];
        self.tabs_meta = tabs_meta;
        self.refreshing_tabs.clear();
    }

    /// Mark one tab as in flight. A ready snapshot stays in place; tabs that
    /// have never succeeded still use the full `Loading` state. Returning
    /// `false` suppresses duplicate requests for the same tab.
    pub fn begin_refresh(&mut self, tab: &TabId) -> bool {
        let Some(index) = self.tabs_meta.iter().position(|current| current == tab) else {
            return false;
        };
        if !self.refreshing_tabs.insert(tab.clone()) {
            return false;
        }
        if !matches!(self.tabs[index], TabState::Ready(_)) {
            self.tabs[index] = TabState::Loading;
        }
        true
    }

    pub fn is_refreshing(&self, tab: &TabId) -> bool {
        self.refreshing_tabs.contains(tab)
    }

    pub fn tab_is_refreshing(&self, index: usize) -> bool {
        self.tabs_meta
            .get(index)
            .is_some_and(|tab| self.is_refreshing(tab))
    }

    /// Apply an asynchronous refresh only when it still belongs to this tab
    /// generation and the captured tab identity still exists. Lookup by
    /// identity, rather than the old positional index, also makes a reordered
    /// tab list safe.
    pub fn apply_refresh(&mut self, generation: u64, tab: &TabId, state: TabState) -> bool {
        if generation != self.tab_generation {
            return false;
        }
        let Some(index) = self.tabs_meta.iter().position(|current| current == tab) else {
            return false;
        };
        let was_refreshing = self.refreshing_tabs.remove(tab);
        // If revalidation fails after a successful snapshot, preserve the
        // useful data but make the failure explicit. Initial failures still
        // become the normal Error state because there is no data to preserve.
        if was_refreshing
            && let TabState::Ready(ready) = &mut self.tabs[index]
            && let TabState::Error(message) = state
        {
            ready.stale = true;
            ready.last_error = Some((0, message));
        } else {
            self.tabs[index] = state;
        }
        true
    }

    /// Move to the first tab of `primary`'s vendor (the default account tab,
    /// since it precedes any of that vendor's account tabs).
    pub fn select_primary(&mut self, primary: Option<VendorId>) {
        if let Some(p) = primary
            && let Some(idx) = self.tabs_meta.iter().position(|t| t.vendor == p)
        {
            self.active = idx;
            self.overview = false;
        }
    }

    /// The selectable ring is `[Overview, tab0, tab1, …]`. `next_tab`/`prev_tab`
    /// walk it, wrapping through the Overview at the ends.
    pub fn next_tab(&mut self) {
        if self.overview {
            if !self.tabs_meta.is_empty() {
                self.overview = false;
                self.active = 0;
            }
        } else if self.active + 1 < self.tabs_meta.len() {
            self.active += 1;
        } else {
            self.overview = true;
        }
    }

    pub fn prev_tab(&mut self) {
        if self.overview {
            if !self.tabs_meta.is_empty() {
                self.overview = false;
                self.active = self.tabs_meta.len() - 1;
            }
        } else if self.active > 0 {
            self.active -= 1;
        } else {
            self.overview = true;
        }
    }

    /// Tabs the Overview should list: `overview_vendors` filtered against the
    /// live tab set (preserving the config order), or all tabs when unset.
    pub fn overview_tabs(&self) -> Vec<usize> {
        match &self.overview_vendors {
            None => (0..self.tabs_meta.len()).collect(),
            Some(wanted) => wanted
                .iter()
                .flat_map(|v| {
                    self.tabs_meta
                        .iter()
                        .enumerate()
                        .filter(move |(_, t)| t.vendor == *v)
                        .map(|(i, _)| i)
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use chrono::{TimeZone, Utc};

    // Use `App::with_theme(.., Theme::default())` rather than `App::new`, which
    // would read the real Omarchy theme file + `$HOME`. The tab-selection logic
    // under test is theme-agnostic.

    #[test]
    fn select_primary_moves_to_enabled_vendor() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openrouter),
            ],
            Theme::default(),
        );
        app.select_primary(Some(VendorId::Openrouter));
        assert_eq!(app.active_vendor(), Some(VendorId::Openrouter));
    }

    #[test]
    fn select_primary_ignores_disabled_vendor() {
        let mut app = App::with_theme(vec![TabId::vendor(VendorId::Anthropic)], Theme::default());
        app.select_primary(Some(VendorId::Openai));
        assert_eq!(app.active_vendor(), Some(VendorId::Anthropic));
    }

    #[test]
    fn nav_ring_wraps_through_the_overview_at_both_ends() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
            ],
            Theme::default(),
        );
        app.overview = true;

        app.next_tab(); // Overview -> first vendor
        assert!(!app.overview);
        assert_eq!(app.active, 0);
        app.next_tab();
        assert_eq!(app.active, 1);
        app.next_tab(); // last vendor -> Overview
        assert!(app.overview);

        app.prev_tab(); // Overview -> last vendor
        assert!(!app.overview);
        assert_eq!(app.active, 1);
        app.prev_tab();
        assert_eq!(app.active, 0);
        app.prev_tab(); // first vendor -> Overview
        assert!(app.overview);
    }

    #[test]
    fn overview_tabs_defaults_to_all_and_honors_the_config_filter() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
                TabId::vendor(VendorId::Zai),
            ],
            Theme::default(),
        );
        assert_eq!(app.overview_tabs(), vec![0, 1, 2]);

        // Subset in the given order.
        app.overview_vendors = Some(vec![VendorId::Zai, VendorId::Anthropic]);
        assert_eq!(app.overview_tabs(), vec![2, 0]);

        // A listed-but-absent vendor is simply skipped.
        app.overview_vendors = Some(vec![VendorId::Grok, VendorId::Openai]);
        assert_eq!(app.overview_tabs(), vec![1]);
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
    fn set_tabs_resets_states_and_clamps_selection() {
        // Simulates a Settings save that shrank the tab list: the selection
        // must clamp into range and every tab must reset to Loading so the
        // caller's spawn_all repopulates against the new config.
        let mut app = App::with_theme(
            tabs_from_config(&config_with_accounts(&["work", "personal"])),
            Theme::default(),
        );
        app.active = 2; // "personal"
        app.tabs[0] = TabState::Error("old".into());
        let old_tab = app.tabs_meta[0].clone();
        assert!(app.begin_refresh(&old_tab));

        app.set_tabs(tabs_from_config(&config_with_accounts(&[])));
        assert_eq!(app.tabs_meta, vec![TabId::vendor(VendorId::Anthropic)]);
        assert_eq!(app.active, 0, "selection clamped after shrink");
        assert!(matches!(app.tabs[0], TabState::Loading));
        assert!(!app.is_refreshing(&old_tab));
    }

    #[test]
    fn set_tabs_preserves_selected_identity_when_entries_are_inserted() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
            ],
            Theme::default(),
        );
        app.active = 1;

        app.set_tabs(vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::account("work"),
            TabId::vendor(VendorId::Openai),
        ]);

        assert_eq!(app.active, 2);
        assert_eq!(app.active_tab_id(), Some(&TabId::vendor(VendorId::Openai)));
    }

    #[test]
    fn refresh_from_old_generation_is_discarded() {
        let mut app = App::with_theme(vec![TabId::vendor(VendorId::Anthropic)], Theme::default());
        let old_generation = app.tab_generation;
        app.set_tabs(vec![TabId::vendor(VendorId::Openai)]);

        assert!(!app.apply_refresh(
            old_generation,
            &TabId::vendor(VendorId::Anthropic),
            TabState::Error("old result".into()),
        ));
        assert!(matches!(app.tabs[0], TabState::Loading));
    }

    #[test]
    fn refresh_identity_mismatch_is_discarded() {
        let mut app = App::with_theme(vec![TabId::vendor(VendorId::Anthropic)], Theme::default());
        let generation = app.tab_generation;

        assert!(!app.apply_refresh(
            generation,
            &TabId::vendor(VendorId::Openai),
            TabState::Error("wrong tab".into()),
        ));
        assert!(matches!(app.tabs[0], TabState::Loading));
    }

    #[test]
    fn refresh_identity_lands_at_new_index_after_same_generation_reorder() {
        let anthropic = TabId::vendor(VendorId::Anthropic);
        let openai = TabId::vendor(VendorId::Openai);
        let mut app = App::with_theme(vec![anthropic.clone(), openai.clone()], Theme::default());
        let generation = app.tab_generation;
        assert!(app.begin_refresh(&anthropic));

        // A reorder is safe because delivery resolves the captured identity,
        // not a stale positional index.
        app.tabs_meta.swap(0, 1);
        app.tabs.swap(0, 1);
        assert!(app.apply_refresh(generation, &anthropic, TabState::Error("ready".into())));
        assert!(matches!(app.tabs[0], TabState::Loading));
        assert!(matches!(&app.tabs[1], TabState::Error(message) if message == "ready"));
        assert!(!app.is_refreshing(&anthropic));
    }

    fn ready_at(fetched_at: chrono::DateTime<Utc>) -> TabState {
        TabState::Ready(Box::new(ReadyTab {
            snapshot: crate::usage::VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
                label: "test".into(),
                total_credits: 0.0,
                total_usage: 0.0,
                usage_daily: 0.0,
                usage_weekly: 0.0,
                usage_monthly: 0.0,
                is_free_tier: false,
                limit: None,
                limit_remaining: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: Some(fetched_at),
        }))
    }

    #[test]
    fn refresh_keeps_ready_snapshot_visible_and_suppresses_duplicates() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let fetched_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        app.tabs[0] = ready_at(fetched_at);

        assert!(app.begin_refresh(&tab));
        assert!(
            !app.begin_refresh(&tab),
            "duplicate request must be suppressed"
        );
        assert!(app.is_refreshing(&tab));
        match &app.tabs[0] {
            TabState::Ready(ready) => assert_eq!(ready.fetched_at, Some(fetched_at)),
            other => panic!("ready snapshot disappeared during refresh: {other:?}"),
        }
    }

    #[test]
    fn first_refresh_still_uses_loading_until_data_arrives() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());

        assert!(app.begin_refresh(&tab));
        assert!(app.is_refreshing(&tab));
        assert!(matches!(app.tabs[0], TabState::Loading));

        assert!(app.apply_refresh(
            app.tab_generation,
            &tab,
            TabState::Error("not signed in".into()),
        ));
        assert!(!app.is_refreshing(&tab));
        assert!(matches!(&app.tabs[0], TabState::Error(message) if message == "not signed in"));
    }

    #[test]
    fn successful_revalidation_replaces_snapshot_and_clears_indicator() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let old_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let new_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 1, 0).unwrap();
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        app.tabs[0] = ready_at(old_at);

        assert!(app.begin_refresh(&tab));
        assert!(app.apply_refresh(app.tab_generation, &tab, ready_at(new_at)));
        assert!(!app.is_refreshing(&tab));
        match &app.tabs[0] {
            TabState::Ready(ready) => assert_eq!(ready.fetched_at, Some(new_at)),
            other => panic!("expected replacement snapshot, got {other:?}"),
        }
    }

    #[test]
    fn failed_revalidation_preserves_snapshot_with_visible_warning() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let fetched_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        app.tabs[0] = ready_at(fetched_at);

        assert!(app.begin_refresh(&tab));
        assert!(app.apply_refresh(
            app.tab_generation,
            &tab,
            TabState::Error("refresh failed".into()),
        ));
        assert!(!app.is_refreshing(&tab));
        match &app.tabs[0] {
            TabState::Ready(ready) => {
                assert_eq!(ready.fetched_at, Some(fetched_at));
                assert!(ready.stale);
                assert_eq!(ready.last_error, Some((0, "refresh failed".into())));
            }
            other => panic!("last successful snapshot was lost: {other:?}"),
        }
        let sections = crate::tui::panels::sections_for(&app.tabs[0], Utc::now(), 5);
        assert!(sections.iter().any(|section| matches!(
            section,
            crate::tui::panels::Section::Text { label, value }
                if label == "Warning" && value == "refresh failed"
        )));
    }

    #[test]
    fn old_generation_result_does_not_clear_current_refresh() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        let old_generation = app.tab_generation;
        app.set_tabs(vec![tab.clone()]);
        assert!(app.begin_refresh(&tab));

        assert!(!app.apply_refresh(old_generation, &tab, TabState::Error("old result".into()),));
        assert!(app.is_refreshing(&tab));
        assert!(matches!(app.tabs[0], TabState::Loading));
    }

    #[test]
    fn apply_refresh_stamps_fetched_at_on_only_the_matching_tab() {
        // Pins the per-tab `fetched_at` the header now reads: a landed Anthropic
        // response leaves the still-loading OpenAI tab with no time of its own.
        // Dropping the global `last_refresh` clock is not observable from here
        // (it was write-only) — that is asserted against the rendered header in
        // `view::tests::header_refresh_*`.
        let anthropic = TabId::vendor(VendorId::Anthropic);
        let openai = TabId::vendor(VendorId::Openai);
        let mut app = App::with_theme(vec![anthropic.clone(), openai], Theme::default());
        let generation = app.tab_generation;
        let fetched_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();

        assert!(app.apply_refresh(generation, &anthropic, ready_at(fetched_at)));
        match &app.tabs[0] {
            TabState::Ready(ready) => assert_eq!(ready.fetched_at, Some(fetched_at)),
            other => panic!("expected Anthropic tab Ready, got {other:?}"),
        }
        assert!(matches!(app.tabs[1], TabState::Loading));
    }

    #[test]
    fn select_primary_lands_on_default_account_tab() {
        // With account tabs present, `primary = anthropic` selects the default
        // Claude tab (index 0), not one of its account tabs.
        let app = {
            let tabs = tabs_from_config(&config_with_accounts(&["work"]));
            let mut a = App::with_theme(tabs, Theme::default());
            a.select_primary(Some(VendorId::Anthropic));
            a
        };
        assert_eq!(app.active, 0);
        assert_eq!(
            app.active_tab_id(),
            Some(&TabId::vendor(VendorId::Anthropic))
        );
    }
}
