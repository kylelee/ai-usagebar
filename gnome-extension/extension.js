// AI Usage Bar — GNOME Shell indicator backed by the `usage --json` report.
//
// The popup tiles ONE TAB PER KEY (account), sorted by vendor & key: each
// tab carries that account's full report rows (header, metric rows with
// band-colored values and meters, live reset countdowns on a 30s ticker,
// text/block rows), an updated-at footer rides below, and a gear button
// in the tab row opens Preferences (its Accounts page manages keys).
// Unconfigured vendors render nothing — only the fresh-install CTA shows
// on an all-unconfigured report — while configured-but-broken accounts
// keep one calm error row below the tabs. The bar button tiles every
// working account: ONE label per tile whose Pango markup colors each
// figure by its own remaining band, a vendor logo when the asset ships,
// wheel/middle-click selection cycling with a persisted choice. Tiling
// off, loading, and nothing-tileable states collapse to the robot icon
// alone.
//
// This file is shell glue only: report parsing and message shaping live in
// report-model.js, binary lookup and spawning in subprocess-utils.js.

import GObject from 'gi://GObject';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

import {Extension, gettext as _} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import {resolveBinary, runJson} from './subprocess-utils.js';
import {
    accountTabs,
    barTileEntries,
    baseProvider,
    errorMessage,
    filteredEntries,
    formatReset,
    formatUpdated,
    isAlarming,
    launchErrorMessage,
    logoAssetName,
    metricDetail,
    parseReport,
    preferredEntryId,
    providerEntryCounts,
    providerName,
    selectedIndex,
    tileParts,
    tileTag,
    windowClass,
    worstBand,
} from './report-model.js';

const ROLE = 'ai-usagebar';

// Force-exit bound for one usage fetch; runJson owns the actual timer.
const REFRESH_TIMEOUT_SECS = 60;
// Countdown/footer cadence between network refreshes (Panel.qml's 30s timer).
const TICKER_SECS = 30;
// Meter geometry: a fixed track; the fill width is computed per percent.
const METER_TRACK_PX = 240;

const Indicator = GObject.registerClass(
class AiUsageBarIndicator extends PanelMenu.Button {
    _init(settings, openPrefs, iconFile, logosDir) {
        super._init(0.0, 'AI Usage Bar', false);

        this._settings = settings;
        this._openPrefs = openPrefs;
        // Async callbacks (the runJson promise, the ticker) never touch a
        // destroyed indicator's actors.
        this._destroyed = false;
        this._report = null;          // last parsed usage report
        this._reportError = '';       // refresh failure shown above the rows
        this._busy = false;
        // A refresh asked for while one was in flight, to run once it settles.
        this._refreshPending = false;
        this._refreshToken = 0;
        this._refreshCancellable = null;
        this._timer = 0;
        this._tickerId = 0;
        this._lastSuccessfulMs = 0;
        this._nowMs = Date.now();
        // Countdown-dependent labels, refreshed in place by the ticker.
        this._countdowns = [];
        this._footerLabel = null;
        this._footerFetchedAt = '';

        // The tiled bar: one tile per working, non-exhausted account. Actors
        // are rebuilt only when the tile entry-id set changes; figures,
        // colors, and countdowns are in-place markup updates.
        this._selectedEntryId = '';
        this._tiles = [];
        this._tileKey = '';
        this._logosDir = logosDir;

        this._barBox = new St.BoxLayout({style: 'spacing: 4px;'});
        this._robotIcon = new St.Icon({
            // Gio.FileIcon has no new_for_file static in newer GJS — constructor form only.
            gicon: new Gio.FileIcon({file: iconFile}),
            icon_size: 16,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-robot',
        });
        this._robotLabel = new St.Label({
            text: '…',
            y_align: Clutter.ActorAlign.CENTER,
        });
        // 2px: omarchy's tile density — its │ separator carries only ~2px
        // pads each side, so tiles read as one tight group inside the plate.
        this._tileBox = new St.BoxLayout({style: 'spacing: 2px;'});
        this._barBox.add_child(this._robotIcon);
        this._barBox.add_child(this._robotLabel);
        this._barBox.add_child(this._tileBox);
        this.add_child(this._barBox);

        this._buildMenu();

        this._intervalId = this._settings.connect('changed::refresh-interval',
            () => this._restartTimer());
        this._sourceIds = [
            // The whole report is always fetched; the vendor key only filters
            // which entries render, so a change re-renders, never refetches.
            this._settings.connect('changed::vendor', () => this._buildMenu()),
            this._settings.connect('changed::binary-path', () => this._refresh()),
            // Preferences bump config-rev after every successful settings
            // apply; the new configuration deserves a fresh fetch.
            this._settings.connect('changed::config-rev', () => this._refresh()),
            // Display toggles re-render the bar in place; no refetch needed.
            this._settings.connect('changed::show-remaining', () => this._updateBar()),
            this._settings.connect('changed::bar-tiled', () => this._updateBar()),
            this._settings.connect('changed::show-logos', () => this._updateBar()),
        ];

        // Wheel cycles the account selection; middle click advances it. The
        // base PanelMenu.Button connected first, so its native primary-click
        // menu toggle runs before these — only button 2 is intercepted, and
        // right-click deliberately has no handler at all.
        this.connect('scroll-event', (_actor, event) => this._onScroll(event));
        this.connect('button-press-event', (_actor, event) => this._onButtonPress(event));

        this.menu.connect('open-state-changed', (_m, open) => {
            if (!open)
                return;
            this._nowMs = Date.now();
            this._onTick();
            // Refresh on open only when the data is older than the interval
            // (the QML panel's staleness rule).
            if (this._lastSuccessfulMs === 0 ||
                    this._nowMs - this._lastSuccessfulMs >= this._intervalSeconds() * 1000)
                this._refresh();
        });

        this._refresh();
        this._restartTimer();
        this._startTicker();
    }

    // The vendor key is a display filter over the whole report: '' = every
    // account, a slug = that vendor's accounts, an exact id = one account.
    _visibleEntries(report = this._report) {
        return report ? filteredEntries(report.entries, this._settings.get_string('vendor')) : [];
    }

    _intervalSeconds() {
        // GSettings already ranges 30–3600; clamping mirrors the QML panel
        // and keeps values from before the range existed honest.
        return Math.max(30, Math.min(3600, this._settings.get_int('refresh-interval')));
    }

    // Selection: keep the current entry while it stays visible, else fall
    // back to the remembered choice, then the report's primary, then the
    // first working account (Panel.qml's syncSelection rule).
    _syncSelection(visible) {
        if (visible.length === 0) {
            this._selectedEntryId = '';
            return;
        }
        if (selectedIndex(visible, this._selectedEntryId) >= 0)
            return;
        this._selectedEntryId = preferredEntryId(visible,
            this._report ? this._report.primary : '',
            this._settings.get_string('last-selected-entry'));
    }

    // Tiles when tiling is on and at least one account is tileable; the
    // robot icon alone otherwise (tiling off, loading, or every account
    // exhausted/broken/unconfigured). The robot glyph never changes
    // appearance — alarms travel through the text color only.
    _updateBar() {
        if (this._destroyed)
            return;
        const visible = this._visibleEntries();
        this._syncSelection(visible);
        const tiled = this._settings.get_boolean('bar-tiled');
        const tiles = this._report
            ? barTileEntries(visible, this._selectedEntryId, tiled) : [];
        const tiledMode = tiled && tiles.length > 0;
        this._robotIcon.visible = !tiledMode;
        this._robotLabel.visible = !tiledMode && !this._report && this._busy;
        this._tileBox.visible = tiledMode;
        // Omarchy-style backdrop plate: wraps the tile row only, never the
        // robot-icon fallback states.
        if (tiledMode)
            this._tileBox.add_style_class_name('aiub-tile-plate');
        else
            this._tileBox.remove_style_class_name('aiub-tile-plate');
        const alarmClass = this._alarming(visible) ? 'aiub-alarm' : '';
        if (!tiledMode) {
            this._robotLabel.style_class = ('aiub-tile-label ' + alarmClass).trim();
            return;
        }
        // Actor churn only when the tile set (or the logo policy) changes;
        // a recovered or exhausted account reshapes the bar on its next
        // report, same set → markup-only update.
        const showLogos = this._settings.get_boolean('show-logos');
        const key = (showLogos ? 'logos|' : '') + tiles.map(entry => entry.id).join('│');
        if (key !== this._tileKey) {
            this._buildTiles(tiles, showLogos);
            this._tileKey = key;
        }
        this._updateTileMarkup(tiles, visible, alarmClass);
    }

    // One tile = optional logo icon + ONE label whose Pango markup carries
    // the tag and every figure; dim separator labels ride between tiles.
    _buildTiles(tiles, showLogos) {
        this._tileBox.destroy_all_children();
        this._tiles = [];
        for (let i = 0; i < tiles.length; i++) {
            const entry = tiles[i];
            if (i > 0)
                this._tileBox.add_child(new St.Label({
                    text: '│',
                    y_align: Clutter.ActorAlign.CENTER,
                    style_class: 'aiub-tile-sep',
                }));
            const tile = new St.BoxLayout({style: 'spacing: 4px;'});
            const asset = showLogos ? logoAssetName(entry.logo) : '';
            if (asset !== '')
                tile.add_child(new St.Icon({
                    gicon: new Gio.FileIcon({file: this._logosDir.get_child(asset)}),
                    icon_size: 16,
                    y_align: Clutter.ActorAlign.CENTER,
                }));
            const label = new St.Label({y_align: Clutter.ActorAlign.CENTER});
            tile.add_child(label);
            this._tileBox.add_child(tile);
            this._tiles.push({id: entry.id, label});
        }
    }

    // The label markup of one tile: the tag stays neutral (theme foreground,
    // inherited from the label), each figure gets a span in its OWN window's
    // remaining-band color read live from the color-* keys, and a critical
    // figure also goes bold so the alert survives color blindness. tileParts
    // text is pre-escaped by the adapter (autoTextSafe); only '&' — the one
    // character it does not neutralize — is encoded here, never re-escaped
    // wholesale (that would show literal entities).
    _tileMarkup(entry, solo) {
        const parts = tileParts(entry, true,
            this._settings.get_boolean('show-remaining'), this._nowMs, solo);
        if (parts.length === 0)
            return '';
        const showLogo = this._settings.get_boolean('show-logos')
            && logoAssetName(entry.logo) !== '';
        const hasTag = tileTag(entry) !== '';
        const segments = [];
        for (let i = 0; i < parts.length; i++) {
            const part = parts[i];
            // Beside a rendered logo only a NAMED account still needs its
            // distinguishing suffix; a default/solo account is the logo alone.
            const text = i === 0 && hasTag && showLogo ? part.logoLabel : part.text;
            if (text === '')
                continue;
            const safe = text.replace(/&/g, '&amp;');
            const body = part.cls === 'critical' ? `<b>${safe}</b>` : safe;
            const hex = this._bandColor(part.cls);
            segments.push(hex !== '' ? `<span foreground="${hex}">${body}</span>` : body);
        }
        return segments.join(' ');
    }

    // Band class → color-low/mid/high/critical key, read live so user
    // overrides land on the next render. A hand-edited dconf value that is
    // not #rrggbb would poison the markup — degrade to neutral instead.
    _bandColor(cls) {
        if (cls === '')
            return '';
        const hex = this._settings.get_string(`color-${cls}`);
        return /^#[0-9a-fA-F]{6}$/.test(hex) ? hex : '';
    }

    // In-place markup refresh: same actors, new figures/colors/countdowns.
    _updateTileMarkup(tiles, visible, alarmClass) {
        const counts = providerEntryCounts(visible);
        for (let i = 0; i < this._tiles.length && i < tiles.length; i++) {
            const entry = tiles[i];
            const solo = counts[baseProvider(entry.id)] <= 1;
            const label = this._tiles[i].label;
            label.clutter_text.set_markup(this._tileMarkup(entry, solo));
            label.style_class = ('aiub-tile-label ' + alarmClass).trim();
        }
    }

    // Urgent states: a refresh failure, a vendor filter matching nothing, or
    // any shown account broken, stale, or critical (worst band included).
    // Colors the bar text through .aiub-alarm — explicit figure spans keep
    // their own colors, so only the neutral tag/ellipsis inherits the red.
    _alarming(visible) {
        if (this._reportError !== '')
            return true;
        if (this._report && visible.length === 0
                && this._settings.get_string('vendor') !== ''
                && this._report.entries.length > 0)
            return true;
        if (worstBand(visible) === 'critical')
            return true;
        for (const entry of visible)
            if (isAlarming(entry))
                return true;
        return false;
    }

    _onScroll(event) {
        if (this._destroyed)
            return Clutter.EVENT_PROPAGATE;
        const direction = event.get_scroll_direction();
        let step = 0;
        if (direction === Clutter.ScrollDirection.UP) {
            step = -1;
        } else if (direction === Clutter.ScrollDirection.DOWN) {
            step = 1;
        } else if (direction === Clutter.ScrollDirection.SMOOTH) {
            // Trackpads emit smooth scrolls; the vertical delta carries the
            // direction.
            const [, dy] = event.get_scroll_delta();
            if (dy !== 0)
                step = dy < 0 ? -1 : 1;
        }
        if (step === 0)
            return Clutter.EVENT_PROPAGATE;
        this._cycleSelection(step);
        return Clutter.EVENT_STOP;
    }

    _onButtonPress(event) {
        if (this._destroyed)
            return Clutter.EVENT_PROPAGATE;
        if (event.get_button() === 2) {
            this._cycleSelection(1);
            return Clutter.EVENT_STOP;
        }
        // Everything else propagates: primary click keeps the native menu
        // toggle and right-click stays unhandled.
        return Clutter.EVENT_PROPAGATE;
    }

    // Cycle over the tileable accounts (ready, non-exhausted) with
    // wraparound — the same population the bar tiles, so the wheel never
    // parks the selection on an entry that cannot show.
    _cycleSelection(step) {
        const visible = this._visibleEntries();
        const cycle = this._report
            ? barTileEntries(visible, this._selectedEntryId, true) : [];
        if (cycle.length === 0)
            return;
        const index = selectedIndex(cycle, this._selectedEntryId);
        const next = ((index + step) % cycle.length + cycle.length) % cycle.length;
        this._applySelection(cycle[next].id);
    }

    _applySelection(entryId) {
        if (entryId === this._selectedEntryId)
            return;
        this._selectedEntryId = entryId;
        this._settings.set_string('last-selected-entry', entryId);
        this._updateBar();
    }

    // The dropdown, synced to the omarchy panel's display logic: a vendor
    // tab row (one tab per vendor that actually reads, plus the gear that
    // opens Preferences), the active tab's account sections, calm error
    // rows for configured-but-broken accounts, and the footer. Full
    // rebuild only when the report (or the filter) changes; the ticker
    // updates countdown-dependent labels in place.
    _buildMenu() {
        this.menu.removeAll();
        this._countdowns = [];
        this._footerLabel = null;
        this._footerFetchedAt = '';

        const entries = this._visibleEntries();

        if (this._reportError !== '')
            this._addMessageRow(this._reportError, 'aiub-error');

        if (!this._report && this._busy && this._reportError === '')
            this._addMessageRow(_('Collecting configured providers…'), 'aiub-note');

        // A vendor filter matching nothing shows its own remedy row; the
        // fresh-install CTA is computed over the UNFILTERED report so an
        // empty filtered view can never trigger it.
        const vendor = this._settings.get_string('vendor');
        const filterMiss = this._report !== null && vendor !== ''
            && this._report.entries.length > 0 && entries.length === 0;
        if (filterMiss)
            this._addFilterMissRow();
        else if (this._report
            && this._report.entries.every(entry => entry.status === 'unconfigured'))
            this._addCtaRow();

        const tabs = this._report ? accountTabs(entries) : [];
        if (tabs.length > 0) {
            this._syncSelection(entries);
            const active = tabs.find(tab => tab.id === this._selectedEntryId) ?? tabs[0];
            if (active.id !== this._selectedEntryId)
                this._applySelection(active.id);
            this._addTabRow(tabs, active);
            this._addEntry(active);
        }

        // Unconfigured vendors render nothing (the user's sync rule — they
        // have no usage to show); an account whose key EXISTS but errors
        // keeps one calm row, because hiding a real failure reads as data.
        for (const entry of entries) {
            if (entry.status === 'error')
                this._addRemedyRow(entry);
        }

        this._addFooter(entries);
        this._updateBar();

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        const refreshItem = new PopupMenu.PopupMenuItem(_('Refresh now'));
        refreshItem.connect('activate', () => this._refresh());
        this.menu.addMenuItem(refreshItem);

        const prefsItem = new PopupMenu.PopupMenuItem(_('Preferences'));
        prefsItem.connect('activate', () => this._openPrefs());
        this.menu.addMenuItem(prefsItem);
    }

    _rowContainer(children) {
        const item = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        for (const child of children)
            item.add_child(child);
        this.menu.addMenuItem(item);
        return item;
    }

    _addMessageRow(text, styleClass) {
        this._rowContainer([new St.Label({text, x_expand: true, style_class: styleClass})]);
    }

    _addCtaRow() {
        const item = new PopupMenu.PopupMenuItem(_('No account configured — open Settings…'));
        item.connect('activate', () => this._openPrefs());
        this.menu.addMenuItem(item);
    }

    // The vendor filter matched no entry: one calm, actionable row replaces
    // the misleading "no account configured" CTA in this state.
    _addFilterMissRow() {
        const item = new PopupMenu.PopupMenuItem(
            _('This filter matches no account — clear it in Preferences.'));
        item.connect('activate', () => this._openPrefs());
        this.menu.addMenuItem(item);
    }

    // One toggle per key (account), vendor & key sorted — plus the gear at
    // the right end: the dropdown's settings entry, opening Preferences
    // whose Accounts page manages API keys. Selecting a tab persists that
    // account as the selection (the same last-selected-entry the wheel
    // cycle writes) and rebuilds the body with that key's usage.
    _addTabRow(tabs, active) {
        const box = new St.BoxLayout({style: 'spacing: 4px;', x_expand: true});
        for (const tab of tabs) {
            const button = new St.Button({
                label: providerName(tab),
                style_class: tab === active
                    ? 'aiub-tab aiub-tab-active' : 'aiub-tab',
                can_focus: true,
                y_align: Clutter.ActorAlign.CENTER,
            });
            button.connect('clicked', () => {
                if (tab === active)
                    return;
                this._applySelection(tab.id);
                this._buildMenu();
            });
            box.add_child(button);
        }
        const gear = new St.Button({
            style_class: 'aiub-tab-gear',
            can_focus: true,
            child: new St.Icon({
                icon_name: 'preferences-system-symbolic',
                icon_size: 14,
                y_align: Clutter.ActorAlign.CENTER,
            }),
            x_expand: true,
            x_align: Clutter.ActorAlign.END,
        });
        gear.connect('clicked', () => this._openPrefs());
        box.add_child(gear);
        this._rowContainer([box]);
    }

    // A configured-but-broken account keeps ONE calm compact row with its
    // remedy — a real failure is information, never alarm-colored.
    _addRemedyRow(entry) {
        const unconfigured = entry.status === 'unconfigured';
        const icon = new St.Icon({
            icon_name: unconfigured ? 'dialog-password-symbolic' : 'dialog-warning-symbolic',
            icon_size: 16,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: unconfigured ? 'aiub-note' : 'aiub-error',
        });
        const name = new St.Label({
            text: providerName(entry),
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-row-name',
        });
        const remedy = new St.Label({
            text: entry.error !== '' ? errorMessage(entry.error)
                : _('Set an API key in Preferences.'),
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-note',
        });
        this._rowContainer([icon, name, remedy]);
    }

    _addEntry(entry) {
        // Header: display_name already carries the account suffix exactly
        // when the provider has siblings (the Rust report owns that rule);
        // the subtitle is the plan, marked cached when data is stale.
        const title = new St.Label({
            text: providerName(entry),
            x_expand: true,
            style_class: 'aiub-header',
        });
        let subText = entry.plan !== '' ? entry.plan : _('Usage and limits');
        if (entry.stale)
            subText += ' · ' + _('cached');
        this._rowContainer([
            title,
            new St.Label({
                text: subText,
                y_align: Clutter.ActorAlign.CENTER,
                style_class: 'aiub-entry-sub',
            }),
        ]);

        for (const section of entry.sections) {
            if (section.type === 'metric')
                this._addMetricRow(section);
            else if (section.type === 'text')
                this._addTextRow(section);
            else if (section.type === 'block')
                this._addBlockRow(section);
            else if (section.type === 'spacer')
                this._addSpacerRow();
        }
    }

    _addMetricRow(section) {
        // Window rows use the same remaining bands as the bar tiles; other
        // metrics keep the Rust severity, whose critical still reads urgent.
        const band = section.window === 'session' || section.window === 'weekly'
            ? windowClass(section) : '';
        const critical = section.severity === 'critical' || band === 'critical';
        const valueClass = band !== ''
            ? `aiub-band-${band}` : (critical ? 'aiub-band-critical' : '');

        const head = new St.BoxLayout({x_expand: true});
        // The warning glyph rides the label so the alert survives color
        // blindness — red alone is not the only channel.
        head.add_child(new St.Label({
            text: (critical ? '󰀪 ' : '') + section.label,
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        }));
        head.add_child(new St.Label({
            text: section.value !== '' ? section.value : `${section.percent}%`,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: ('aiub-metric-value ' + valueClass).trim(),
        }));

        // Meter: fixed-width horizontal St.BoxLayout track, the fill packed
        // as its first child — BoxLayout children stack from x=0, so the
        // progress anchors LEFT by construction and never depends on the
        // child-alignment semantics of St.Bin/ClutterBinLayout (whose
        // centered default floated the fill as a stub). Colors come from
        // stylesheet classes so the tiled bar can extend the same palette.
        const track = new St.BoxLayout({
            style_class: 'aiub-meter-track',
            width: METER_TRACK_PX,
            x_align: Clutter.ActorAlign.START,
        });
        const fillPx = Math.max(0, Math.min(METER_TRACK_PX,
            Math.round(section.percent * METER_TRACK_PX / 100)));
        const fill = new St.Bin({
            style: `width: ${fillPx}px;`,
            style_class: ('aiub-meter-fill ' + valueClass).trim(),
        });
        track.add_child(fill);

        const vbox = new St.BoxLayout({
            orientation: Clutter.Orientation.VERTICAL,
            x_expand: true,
            style_class: 'aiub-row',
        });
        vbox.add_child(head);
        vbox.add_child(track);

        const detailText = metricDetail(section);
        if (detailText !== '')
            vbox.add_child(new St.Label({text: detailText, style_class: 'aiub-detail'}));

        // Live reset countdown with the local clock and cross-day dating.
        const reset = new St.Label({style_class: 'aiub-detail'});
        reset.text = formatReset(section.reset_at, this._nowMs);
        reset.visible = reset.text !== '';
        vbox.add_child(reset);
        if (section.reset_at !== '')
            this._countdowns.push({label: reset, resetAt: section.reset_at});

        this._rowContainer([vbox]);
    }

    _addTextRow(section) {
        if (section.value === '') {
            // Heading: all-caps, like the QML panel's section titles.
            this._rowContainer([new St.Label({
                text: section.label.toUpperCase(),
                x_expand: true,
                style_class: 'aiub-header',
            })]);
            return;
        }
        const box = new St.BoxLayout({x_expand: true, style: 'spacing: 8px;'});
        box.add_child(new St.Label({
            text: section.label,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-row-name',
        }));
        box.add_child(new St.Label({
            text: section.value,
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-entry-sub',
        }));
        this._rowContainer([box]);
    }

    _addBlockRow(section) {
        const vbox = new St.BoxLayout({
            orientation: Clutter.Orientation.VERTICAL,
            x_expand: true,
            style_class: 'aiub-row',
        });
        vbox.add_child(new St.Label({text: section.label, style_class: 'aiub-row-name'}));
        if (section.body.length > 0)
            vbox.add_child(new St.Label({
                text: section.body.join('\n'),
                style_class: 'aiub-detail',
            }));
        this._rowContainer([vbox]);
    }

    _addSpacerRow() {
        this._rowContainer([new St.BoxLayout({style_class: 'aiub-spacer', x_expand: true})]);
    }

    _addFooter(entries) {
        // Data age across the merged list: the newest entry fetched_at.
        let latest = '';
        for (const entry of entries)
            if (entry.fetched_at !== '' && entry.fetched_at > latest)
                latest = entry.fetched_at;
        if (latest === '')
            return;
        this._footerFetchedAt = latest;
        this._footerLabel = new St.Label({
            x_expand: true,
            x_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-note',
        });
        this._rowContainer([this._footerLabel]);
        this._updateFooter(this._nowMs);
    }

    _updateFooter(nowMs) {
        if (!this._footerLabel)
            return;
        const parts = [];
        if (this._footerFetchedAt !== '')
            parts.push(formatUpdated(this._footerFetchedAt, nowMs));
        if (this._busy)
            parts.push(_('refreshing…'));
        this._footerLabel.text = parts.join(' · ');
    }

    // Recompute ONLY the reset strings and the footer — no menu rebuild, no
    // actor churn every 30 seconds. The bar update recomputes tile markup in
    // place so the embedded compact countdowns stay live.
    _onTick() {
        if (this._destroyed)
            return;
        this._nowMs = Date.now();
        for (const countdown of this._countdowns) {
            countdown.label.text = formatReset(countdown.resetAt, this._nowMs);
            countdown.label.visible = countdown.label.text !== '';
        }
        this._updateFooter(this._nowMs);
        this._updateBar();
    }

    _startTicker() {
        if (this._tickerId)
            return;
        this._tickerId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, TICKER_SECS, () => {
            this._onTick();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _restartTimer() {
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, this._intervalSeconds(), () => {
            this._refresh();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _refresh() {
        // Dropping the request while busy meant a change *during* a fetch
        // never started one for the new state: the in-flight result settled
        // and nothing followed. Remember that a refresh was asked for and
        // run it as soon as the current one settles.
        if (this._busy) {
            this._refreshPending = true;
            return;
        }
        this._busy = true;
        const token = ++this._refreshToken;

        this._updateFooter(this._nowMs);

        const cancellable = new Gio.Cancellable();
        this._refreshCancellable = cancellable;

        runJson([resolveBinary(this._settings), 'usage', '--json'],
            {timeoutSec: REFRESH_TIMEOUT_SECS, cancellable})
            .then(res => {
                if (this._destroyed)
                    return;
                const current = this._refreshToken === token;
                if (current) {
                    this._busy = false;
                    this._handleResult(res);
                }
                if (current && this._refreshPending) {
                    this._refreshPending = false;
                    this._refresh();
                }
            });
    }

    _handleResult(res) {
        if (res.stdout.trim() !== '') {
            // Parse stdout regardless of exit code: a fully-unconfigured
            // report still exits non-zero and still renders (the QML panel's
            // finishRefresh semantics).
            const parsed = parseReport(res.stdout);
            if (parsed.ok) {
                this._report = parsed;
                this._reportError = '';
                this._lastSuccessfulMs = Date.now();
                this._buildMenu();
                return;
            }
            const detail = res.stderr.trim() !== ''
                ? errorMessage(res.stderr)
                : res.error !== '' ? res.error : parsed.error;
            this._reportError = this._previousReportNotice(detail);
            this._buildMenu();
            return;
        }
        let message;
        if (res.error.startsWith('spawn failed') || res.exitCode === 127) {
            // Gio throws when the binary is missing (no /usr/bin/env shim
            // here) — surface the same install remedy the QML model gives a
            // 127 exit instead of a raw spawn error.
            message = launchErrorMessage(127, res.stderr);
        } else if (res.error !== '') {
            message = res.error;
        } else {
            message = errorMessage(res.stderr);
        }
        this._reportError = this._report
            ? this._previousReportNotice(message) : message;
        this._buildMenu();
    }

    _previousReportNotice(detail) {
        const base = _('Refresh failed; showing the previous report.');
        return detail !== '' ? `${base} ${detail}` : base;
    }

    destroy() {
        this._destroyed = true;
        this._busy = false;
        this._refreshPending = false;
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        if (this._tickerId) {
            GLib.source_remove(this._tickerId);
            this._tickerId = 0;
        }
        if (this._refreshCancellable) {
            this._refreshCancellable.cancel();
            this._refreshCancellable = null;
        }
        for (const id of this._sourceIds ?? [])
            this._settings.disconnect(id);
        if (this._intervalId)
            this._settings.disconnect(this._intervalId);
        this._sourceIds = null;
        this._intervalId = 0;
        this._countdowns = [];
        this._footerLabel = null;
        this._tiles = [];
        this._tileKey = '';
        this._tileBox = null;
        this._robotIcon = null;
        this._robotLabel = null;
        super.destroy();
    }
});

export default class AiUsageBarExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._place();
        this._placeIds = [
            this._settings.connect('changed::panel-box', () => this._place()),
            this._settings.connect('changed::panel-index', () => this._place()),
        ];
    }

    _place() {
        const existing = Main.panel.statusArea[ROLE];
        if (existing) {
            existing.destroy();
            delete Main.panel.statusArea[ROLE];
        }
        const iconFile = this.dir.get_child('icons').get_child('ai-usagebar-symbolic.svg');
        const logosDir = this.dir.get_child('logos');
        this._indicator = new Indicator(this._settings, () => this.openPreferences(),
            iconFile, logosDir);
        const box = this._settings.get_string('panel-box') || 'right';
        const index = Math.max(0, this._settings.get_int('panel-index'));
        Main.panel.addToStatusArea(ROLE, this._indicator, index, box);
    }

    disable() {
        for (const id of this._placeIds ?? [])
            this._settings.disconnect(id);
        this._placeIds = null;
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
        delete Main.panel.statusArea[ROLE];
        this._settings = null;
    }
}
