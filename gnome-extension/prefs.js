// Preferences (libadwaita) for AI Usage Bar.

import Adw from 'gi://Adw';
import Gtk from 'gi://Gtk';
import Gdk from 'gi://Gdk';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {ExtensionPreferences, gettext as _} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import {resolveBinary, runJson, runWithStdin} from './subprocess-utils.js';
import {
    baseProvider,
    buildSettingsPatch,
    launchErrorMessage,
    parseReport,
    parseSettingsApplyResult,
    parseSettingsSnapshot,
    providerName,
    VENDOR_SLUGS,
} from './report-model.js';

// Login flows that live OUTSIDE the settings bridge: the two OAuth CLIs
// manage their own credential files, and Antigravity is a local server with
// no separate login at all. Everything else on the Accounts page is
// data-driven from `settings show` — these three stay because launching a
// terminal login is a frontend-owned action, not report data.
const LOGIN_FLOWS = [
    {id: 'anthropic', name: 'Claude', kind: 'oauth', cli: 'claude', login: 'claude', pkg: '@anthropic-ai/claude-code'},
    {id: 'openai', name: 'Codex', kind: 'oauth', cli: 'codex', login: 'codex login', pkg: '@openai/codex'},
    // Local-server vendor: there is no separate credential or npm-installable
    // login helper. If `agy` exists we can open it; the app and IDE are equally
    // valid sources and are managed outside this extension.
    {id: 'antigravity', name: 'Google Antigravity', kind: 'local', cli: 'agy'},
];

// File-based state for the login flows only. Key-based vendors are NOT probed
// here — their presence booleans come from the settings bridge.
function loginVendorConfigured(v) {
    const home = GLib.get_home_dir();
    if (v.id === 'anthropic')
        return GLib.file_test(`${home}/.claude/.credentials.json`, GLib.FileTest.EXISTS);
    if (v.id === 'openai')
        return GLib.file_test(`${home}/.codex/auth.json`, GLib.FileTest.EXISTS);
    // Antigravity 2.0, the `agy` CLI and the IDE are separate products sharing
    // one quota, and any combination may be installed. Having any of their
    // state directories is enough — the binary reads usage from whichever
    // local server is running, so there is no credential file to look for.
    return ['antigravity', 'antigravity-cli', 'antigravity-ide']
        .some(d => GLib.file_test(`${home}/.gemini/${d}`, GLib.FileTest.IS_DIR));
}

// Open the user's terminal running `command` (login shell, so claude/codex are on PATH).
function spawnInTerminal(command) {
    for (const argv of [
        ['kgx', '--', 'bash', '-lc', command],
        ['gnome-terminal', '--', 'bash', '-lc', command],
        ['xterm', '-e', 'bash', '-lc', command],
    ]) {
        if (!GLib.find_program_in_path(argv[0]))
            continue;
        try {
            Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
            return true;
        } catch (e) {}
    }
    return false;
}

function spawnArgvInTerminal(commandArgv) {
    for (const argv of [
        ['kgx', '--', ...commandArgv],
        ['gnome-terminal', '--', ...commandArgv],
        ['xterm', '-e', ...commandArgv],
    ]) {
        if (!GLib.find_program_in_path(argv[0]))
            continue;
        try {
            Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
            return true;
        } catch (e) {}
    }
    return false;
}

// The terminal-settings fallback for a binary that predates the settings
// bridge — same resolution chain the old vendor page used.
function spawnTui() {
    const tui = GLib.find_program_in_path('ai-usagebar-omarchy-tui') ||
        GLib.find_program_in_path('ai-usagebar-tui') ||
        `${GLib.get_home_dir()}/.cargo/bin/ai-usagebar-omarchy-tui`;
    spawnArgvInTerminal([tui]);
}

// Is `cli` on the login-shell PATH? (npm-global / nvm bins often aren't on the
// prefs process PATH, so we ask a login shell.)
function checkCliInstalled(cli, callback) {
    try {
        const p = Gio.Subprocess.new(
            ['bash', '-lc', `command -v ${cli}`],
            Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_SILENCE);
        p.communicate_utf8_async(null, null, (proc, res) => {
            try {
                const [, out] = proc.communicate_utf8_finish(res);
                callback((out || '').trim().length > 0);
            } catch (e) {
                callback(false);
            }
        });
    } catch (e) {
        callback(false);
    }
}

// Terminal script: if the CLI is missing, offer to npm-install it (with
// consent), then run the login. Runs in a login shell so PATH is complete.
function oauthCommand(v) {
    // Installs to ~/.local (already on PATH) so no sudo is needed even when the
    // system npm prefix is root-owned (/usr/lib/node_modules).
    return [
        `export PATH="$HOME/.local/bin:$PATH";`,
        `if command -v ${v.cli} >/dev/null 2>&1; then ${v.login};`,
        `else echo "⚠ ${v.cli} not found.";`,
        `echo "Install to ~/.local without sudo (npm --prefix). Package: ${v.pkg}"; echo;`,
        `read -p "Install now? [y/N] " a;`,
        `if [ "$a" = y ] || [ "$a" = Y ]; then npm i -g --prefix "$HOME/.local" ${v.pkg} && hash -r && ${v.login}; fi;`,
        `fi;`,
        `echo; read -p "Press Enter to close..."`,
    ].join(' ');
}

// Display label for one choice value; Rust owns the canonical names via the
// snapshot's labels map, '' degrades to "auto".
function choiceLabel(field, choice) {
    const label = field.labels && field.labels[choice] !== undefined
        ? field.labels[choice] : choice;
    return label === '' ? 'auto' : label;
}

function teamIdsRemedy() {
    return _('A team key needs both ids: bigmodel.cn console → F12 → Application → Local Storage → Bigmodel-Organization / Bigmodel-Project.');
}

// Bare provider label for the vendor filter's provider rows: the report
// ships display_name with the account suffix appended ("Z.AI · team"), but
// the provider row means "every account of this provider". The suffix is
// stripped only when the entry's own id suffix (plus the optional desktop
// marker) actually matches the tail, so a label containing " · " cannot
// make the strip land in the middle of the provider name.
function vendorBaseLabel(entry) {
    const id = String(entry.id);
    const at = id.indexOf('@');
    const name = providerName(entry);
    if (at < 0)
        return name;
    const suffix = id.slice(at + 1);
    for (const tail of [` · ${suffix}`, ` · ${suffix} (desktop)`]) {
        if (name.length > tail.length && name.endsWith(tail))
            return name.slice(0, name.length - tail.length);
    }
    return name;
}

// Repopulate the vendor filter from a live report, mirroring exactly how
// the panel filters: '' tiles every account, a provider id shows that
// vendor's accounts, an exact entry id pins one account. Providers come
// first in report order, then one row per named account entry (a lone,
// suffix-less entry is already covered by its provider row). The
// subprocess is ASYNC and runJson never rejects, so the dialog never
// blocks or crashes on it; the exit status is deliberately ignored — a
// report in which every vendor is unconfigured still exits non-zero yet
// names every account, which is exactly when the picker is needed most.
// Any failure (binary missing, empty stdout, unparseable report) keeps
// the static slug list the row starts with.
function refreshVendorFilter(settings, repopulate, isClosed) {
    runJson([resolveBinary(settings), 'usage', '--json'], {timeoutSec: 60})
        .then(res => {
            if (isClosed())
                return;
            const stdout = String(res.stdout ?? '');
            if (stdout.trim() === '')
                return;
            const parsed = parseReport(stdout);
            if (!parsed.ok || parsed.entries.length === 0)
                return;
            const values = [''];
            const labels = [_('All accounts')];
            const seen = new Set();
            for (const entry of parsed.entries) {
                const base = baseProvider(entry.id);
                if (seen.has(base))
                    continue;
                seen.add(base);
                values.push(base);
                labels.push(vendorBaseLabel(entry));
            }
            for (const entry of parsed.entries) {
                if (!String(entry.id).includes('@'))
                    continue;
                values.push(entry.id);
                labels.push(providerName(entry));
            }
            const current = settings.get_string('vendor');
            if (current !== '' && !values.includes(current)) {
                // The stored filter names a provider or account this report
                // no longer has; the row would show "All accounts" while
                // the stale id kept the panel empty. Clear it for real.
                settings.set_string('vendor', '');
            }
            repopulate(values, labels);
        });
}

// Bind an Adw.ComboRow (index-based) to a string GSetting via a value table.
// Returns repopulate(values, labels): swap the row's list without writing
// the transient model-reset selection back to the setting.
function bindCombo(settings, key, comboRow, values, labels = values) {
    let populating = false;
    const sync = () => {
        const idx = values.indexOf(settings.get_string(key));
        comboRow.selected = idx < 0 ? 0 : idx;
    };
    sync();
    comboRow.connect('notify::selected', () => {
        if (populating)
            return;
        const v = values[comboRow.selected];
        if (v !== undefined && v !== settings.get_string(key))
            settings.set_string(key, v);
    });
    settings.connect(`changed::${key}`, sync);
    return (nextValues, nextLabels = nextValues) => {
        populating = true;
        values.length = 0;
        values.push(...nextValues);
        labels.length = 0;
        labels.push(...nextLabels);
        comboRow.model = Gtk.StringList.new(labels);
        sync();
        populating = false;
    };
}

function rgbaToHex(rgba) {
    const h = v => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, '0');
    return `#${h(rgba.red)}${h(rgba.green)}${h(rgba.blue)}`;
}

// A row with a GTK color picker bound to a hex-string GSetting.
function colorRow(settings, key, title) {
    const row = new Adw.ActionRow({title});
    const btn = new Gtk.ColorDialogButton({
        dialog: new Gtk.ColorDialog({with_alpha: false}),
        valign: Gtk.Align.CENTER,
    });
    let updating = false;
    const load = () => {
        const rgba = new Gdk.RGBA();
        if (rgba.parse(settings.get_string(key))) {
            updating = true;
            btn.set_rgba(rgba);
            updating = false;
        }
    };
    load();
    btn.connect('notify::rgba', () => {
        if (!updating)
            settings.set_string(key, rgbaToHex(btn.get_rgba()));
    });
    settings.connect(`changed::${key}`, load);
    row.add_suffix(btn);
    row.activatable_widget = btn;
    return row;
}

export default class AiUsageBarPrefs extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();
        this._settings = settings;
        this._window = window;
        // Async subprocess callbacks must never touch widgets after the
        // dialog closed — a destroyed widget throws in GJS.
        this._closed = false;
        window.connect('close-request', () => {
            this._closed = true;
        });

        const page = new Adw.PreferencesPage({title: _('Common')});
        window.add(page);

        // ── Display ──────────────────────────────────────────────────────
        const display = new Adw.PreferencesGroup({title: _('Display')});
        page.add(display);

        const showRemaining = new Adw.SwitchRow({
            title: _('Show remaining'),
            subtitle: _('Show what is left of each window in the tiles instead of the used percentage'),
        });
        settings.bind('show-remaining', showRemaining, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showRemaining);

        const barTiled = new Adw.SwitchRow({
            title: _('Tile accounts'),
            subtitle: _('Every working account side by side; off shows only the selected account'),
        });
        settings.bind('bar-tiled', barTiled, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(barTiled);

        const showLogos = new Adw.SwitchRow({
            title: _('Show logos'),
            subtitle: _('Lead each bar tile with its provider logo'),
        });
        settings.bind('show-logos', showLogos, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showLogos);

        // ── Colors ───────────────────────────────────────────────────────
        const colors = new Adw.PreferencesGroup({
            title: _('Colors'),
            description: _('Figure color by remaining band (One Dark by default).'),
        });
        page.add(colors);
        colors.add(colorRow(settings, 'color-low', _('Low (at least 50% remaining)')));
        colors.add(colorRow(settings, 'color-mid', _('Medium (below 50% remaining)')));
        colors.add(colorRow(settings, 'color-high', _('High (below 20% remaining)')));
        colors.add(colorRow(settings, 'color-critical', _('Critical (below 5% remaining)')));
        colors.add(colorRow(settings, 'color-empty', _('Empty (meter track)')));

        // ── Data ─────────────────────────────────────────────────────────
        const data = new Adw.PreferencesGroup({title: _('Data')});
        page.add(data);

        const interval = new Adw.SpinRow({
            title: _('Refresh interval (s)'),
            adjustment: new Gtk.Adjustment({lower: 30, upper: 3600, step_increment: 5, page_increment: 30}),
        });
        settings.bind('refresh-interval', interval, 'value', Gio.SettingsBindFlags.DEFAULT);
        data.add(interval);

        // Static slugs first so the filter row is never empty before the
        // live report resolves (and stays usable when the binary is
        // missing); refreshVendorFilter below swaps in the report's
        // providers and account entries.
        const vendorValues = ['', ...VENDOR_SLUGS];
        const vendorLabels = [_('All accounts'), ...VENDOR_SLUGS];
        const vendor = new Adw.ComboRow({
            title: _('Vendor filter'),
            subtitle: _('All accounts, all accounts of one provider, or a single account'),
            model: Gtk.StringList.new(vendorLabels),
        });
        const repopulateVendor = bindCombo(settings, 'vendor', vendor, vendorValues, vendorLabels);
        data.add(vendor);
        refreshVendorFilter(settings, repopulateVendor, () => this._closed);

        const binPath = new Adw.EntryRow({title: _('Binary path (empty = auto)')});
        settings.bind('binary-path', binPath, 'text', Gio.SettingsBindFlags.DEFAULT);
        data.add(binPath);

        // ── Position ─────────────────────────────────────────────────────
        const pos = new Adw.PreferencesGroup({
            title: _('Panel position'),
            description: _('Changes apply immediately.'),
        });
        page.add(pos);

        const box = new Adw.ComboRow({
            title: _('Area'),
            subtitle: _('right = next to the network/clock'),
            model: Gtk.StringList.new(['left', 'center', 'right']),
        });
        bindCombo(settings, 'panel-box', box, ['left', 'center', 'right']);
        pos.add(box);

        const index = new Adw.SpinRow({
            title: _('Index within the area'),
            subtitle: _('0 = leftmost in the chosen area'),
            adjustment: new Gtk.Adjustment({lower: 0, upper: 20, step_increment: 1, page_increment: 1}),
        });
        settings.bind('panel-index', index, 'value', Gio.SettingsBindFlags.DEFAULT);
        pos.add(index);

        this._buildAccountsPage(window);
    }

    // ── Accounts page: data-driven over the settings bridge ─────────────
    // The Rust binary owns the configuration; this page only projects the
    // `settings show` snapshot and sends per-card mutations over stdin
    // (`settings apply`). No configuration file is ever read here.

    _buildAccountsPage(window) {
        const page = new Adw.PreferencesPage({
            title: _('Accounts'),
            icon_name: 'dialog-password-symbolic',
        });
        window.add(page);
        this._page = page;
        this._dynamic = [];
        this._applyButtons = [];
        this._loginUpdates = [];
        this._snapshot = null;
        this._loginGroupRef = null;
        this._primaryRow = null;
        this._applyBusy = false;
        this._renderGen = 0;

        // Re-check login states when the window regains focus (e.g., after
        // logging in through the terminal).
        window.connect('notify::is-active', () => {
            if (window.is_active && !this._closed)
                this._loginUpdates.forEach(update => update());
        });

        this._renderPage(null, '');
        this._reloadSnapshot();
    }

    _reloadSnapshot() {
        runJson([resolveBinary(this._settings), 'settings', 'show'], {timeoutSec: 30})
            .then(res => {
                if (this._closed)
                    return;
                if (!res.ok) {
                    const legacy = res.stderr.includes('unrecognized subcommand') ||
                        (res.error === '' && res.exitCode !== 0 && res.stdout === '');
                    const message = legacy
                        ? _('The installed ai-usagebar-omarchy binary predates native settings. Update the package, or use the terminal settings fallback.')
                        : res.error !== '' ? res.error : launchErrorMessage(res.exitCode, res.stderr);
                    this._renderPage(null, message);
                    return;
                }
                const parsed = parseSettingsSnapshot(res.stdout);
                if (!parsed.ok) {
                    this._renderPage(null, parsed.error);
                    return;
                }
                this._renderPage(parsed, '');
            });
    }

    _renderPage(parsed, errorMessage) {
        for (const group of this._dynamic)
            this._page.remove(group);        this._dynamic = [];
        this._applyButtons = [];
        this._loginUpdates = [];
        this._primaryRow = null;
        this._renderGen++;
        this._snapshot = parsed;
        this._applyBusy = false;
        if (errorMessage !== '')
            this._push(this._bannerGroup(errorMessage));
        if (parsed) {
            if (parsed.primary_choices.length > 0)
                this._push(this._primaryGroup(parsed));
            for (const group of this._vendorGroups(parsed))
                this._push(group);
        }
        // Login flows stay reachable in every state, including load failure.
        this._loginGroupRef = this._loginGroup();
        this._push(this._loginGroupRef);
    }

    _push(group) {
        this._page.add(group);
        this._dynamic.push(group);
    }

    // Inline error surface with the two recoveries: retry the bridge, or
    // fall back to the terminal settings TUI.
    _bannerGroup(message) {
        const group = new Adw.PreferencesGroup({
            title: _('Settings bridge'),
            description: message,
        });
        const row = new Adw.ActionRow();
        const retry = new Gtk.Button({label: _('Retry'), valign: Gtk.Align.CENTER});
        retry.connect('clicked', () => this._reloadSnapshot());
        const terminal = new Gtk.Button({
            label: _('Open terminal settings'),
            valign: Gtk.Align.CENTER,
        });
        terminal.connect('clicked', () => spawnTui());
        row.add_suffix(retry);
        row.add_suffix(terminal);
        group.add(row);
        return group;
    }

    // Page-top primary selector; a change applies immediately as its own
    // single-field patch (NOT a save-all).
    _primaryGroup(parsed) {
        const group = new Adw.PreferencesGroup({
            title: _('Primary provider'),
            description: _('Used by the CLI, the bar widgets, and the TUI on startup.'),
        });
        const values = parsed.primary_choices.map(choice => choice.id);
        const labels = parsed.primary_choices.map(choice => choice.label);
        const combo = new Adw.ComboRow({model: Gtk.StringList.new(labels)});
        const current = Math.max(0, values.indexOf(parsed.primary));
        this._primaryRow = combo;
        combo.selected = current;
        combo.connect('notify::selected', () => {
            if (this._applyBusy)
                return;
            const value = values[combo.selected];
            if (value === undefined || value === parsed.primary)
                return;
            const built = buildSettingsPatch(value, [], []);
            if (!built.ok)
                return;
            this._setApplyBusy(true);
            runWithStdin([resolveBinary(this._settings), 'settings', 'apply'], built.payload,
                {timeoutSec: 30})
                .then(res => {
                    if (this._closed)
                        return;
                    if (res.ok && parseSettingsApplyResult(res.stdout)) {
                        // Refresh ping: the extension watches config-rev.
                        this._settings.set_int('config-rev',
                            this._settings.get_int('config-rev') + 1);
                        this._reloadSnapshot();
                        return;
                    }
                    this._setApplyBusy(false);
                    combo.selected = current;
                    this._showBanner(this._applyFailureText(res));
                });
        });
        group.add(combo);
        return group;
    }

    // One group per vendor (snapshot order), each with its cards and its own
    // Add-account entry point. Rust owns the grouping and the display names.
    _vendorGroups(parsed) {
        const vendors = [];
        const byVendor = new Map();
        for (const card of parsed.accounts) {
            let entry = byVendor.get(card.vendor);
            if (!entry) {
                entry = {vendor: card.vendor, display: card.vendor_display, cards: []};
                byVendor.set(card.vendor, entry);
                vendors.push(entry);
            }
            entry.cards.push(card);
        }
        return vendors.map(entry => {
            const group = new Adw.PreferencesGroup({title: entry.display});
            for (const card of entry.cards)
                group.add(this._accountCard(card));
            group.add(this._addAccountRow(group, entry));
            return group;
        });
    }

    _addAccountRow(group, entry) {
        const row = new Adw.ActionRow();
        const btn = new Gtk.Button({
            label: _('Add %s account').format(entry.display),
            valign: Gtk.Align.CENTER,
        });
        btn.connect('clicked', () => {
            if (this._applyBusy)
                return;
            group.add(this._draftCard(entry));
        });
        row.add_suffix(btn);
        return row;
    }

    _accountCard(card) {
        const row = new Adw.ExpanderRow({
            title: card.label === '' ? card.vendor_display
                : `${card.vendor_display} · ${card.display}`,
        });
        const state = {keyText: '', keyClear: false};
        const fieldGets = {};
        const knownValues = {};
        for (const field of card.fields)
            knownValues[field.id] = String(field.value || '');
        const snapshotIds = new Set(card.fields.map(field => field.id));
        let typeGet = null;
        let orgEntry = null;
        let projEntry = null;

        const remedyRow = new Adw.ActionRow({title: teamIdsRemedy(), visible: false, title_lines: 3});
        remedyRow.add_css_class('error');
        const errorRow = new Adw.ActionRow({visible: false, title_lines: 3});
        errorRow.add_css_class('error');
        const showError = text => {
            errorRow.title = text;
            errorRow.visible = text !== '';
        };

        const syncStatus = () => {
            row.subtitle = state.keyClear ? _('key will clear')
                : state.keyText.trim() !== '' ? _('new key')
                : card.environment_configured ? _('environment override')
                : card.inline_configured ? _('key stored')
                : _('no key');
        };
        syncStatus();

        // The team-id inputs exist only for accounts already typed "team" in
        // the snapshot; a personal card flipped to team mid-edit gets
        // synthesized ones below.
        function syncTeamState() {
            const team = card.vendor === 'zai' && typeGet !== null && typeGet() === 'team';
            if (orgEntry)
                orgEntry.visible = team;
            if (projEntry)
                projEntry.visible = team;
            const org = orgEntry ? String(orgEntry.text ?? '') : '';
            const proj = projEntry ? String(projEntry.text ?? '') : '';
            remedyRow.visible = team && (org.trim() === '' || proj.trim() === '');
        }

        for (const field of card.fields) {
            if (field.kind === 'choice') {
                const values = field.choices.length > 0 ? field.choices : [field.value || ''];
                const labels = values.map(choice => choiceLabel(field, choice));
                const combo = new Adw.ComboRow({
                    title: field.label,
                    model: Gtk.StringList.new(labels),
                });
                const idx = values.indexOf(field.value);
                combo.selected = idx >= 0 ? idx : 0;
                fieldGets[field.id] = () => values[combo.selected] ?? values[0];
                if (field.id === 'account_type')
                    typeGet = fieldGets[field.id];
                combo.connect('notify::selected', syncTeamState);
                row.add_row(combo);
            } else if (field.kind !== 'secret') {
                const entry = new Adw.EntryRow({title: field.label, text: field.value});
                fieldGets[field.id] = () => String(entry.text ?? '');
                if (field.id === 'organization_id')
                    orgEntry = entry;
                if (field.id === 'project_id')
                    projEntry = entry;
                entry.connect('notify::text', syncTeamState);
                row.add_row(entry);
            }
        }
        if (card.vendor === 'zai') {
            if (!snapshotIds.has('organization_id')) {
                orgEntry = new Adw.EntryRow({title: _('Organization ID')});
                row.add_row(orgEntry);
            }
            if (!snapshotIds.has('project_id')) {
                projEntry = new Adw.EntryRow({title: _('Project ID')});
                row.add_row(projEntry);
            }
        }

        // The secret row: blank NEVER means clear — that is the explicit
        // button next to it, sensitive only while an inline key exists.
        // The peek icon: libadwaita 2.0 removed show-peek-icon (the eye is
        // always shown); set it only where the property still exists.
        const keyEntry = new Adw.PasswordEntryRow({
            title: card.configured ? _('Leave blank to keep current key') : _('Paste API key'),
        });
        if (typeof keyEntry.show_peek_icon === 'boolean')
            keyEntry.show_peek_icon = true;
        keyEntry.connect('notify::text', () => {
            state.keyText = String(keyEntry.text ?? '');
            syncStatus();
        });
        const clearBtn = new Gtk.Button({valign: Gtk.Align.CENTER});
        clearBtn.add_css_class('flat');
        const syncClear = () => {
            clearBtn.label = state.keyClear ? _('Undo clear') : _('Clear key');
            clearBtn.sensitive = card.inline_configured || state.keyClear;
            keyEntry.sensitive = !state.keyClear;
        };
        clearBtn.connect('clicked', () => {
            state.keyClear = !state.keyClear;
            if (state.keyClear)
                keyEntry.text = '';
            syncClear();
            syncStatus();
        });
        keyEntry.add_suffix(clearBtn);
        syncClear();
        row.add_row(keyEntry);

        row.add_row(remedyRow);
        row.add_row(errorRow);

        // Only actual changes travel: fields that differ from the snapshot,
        // synthesized team ids when non-empty, and a key action only when
        // the user typed one or armed the explicit clear.
        function collect() {
            const fields = {};
            for (const field of card.fields) {
                if (field.kind === 'secret')
                    continue;
                const get = fieldGets[field.id];
                if (!get)
                    continue;
                const current = String(get() ?? '');
                if (current !== knownValues[field.id])
                    fields[field.id] = current;
            }
            if (card.vendor === 'zai' && !snapshotIds.has('organization_id')
                && orgEntry && String(orgEntry.text ?? '').trim() !== '')
                fields.organization_id = String(orgEntry.text ?? '');
            if (card.vendor === 'zai' && !snapshotIds.has('project_id')
                && projEntry && String(projEntry.text ?? '').trim() !== '')
                fields.project_id = String(projEntry.text ?? '');
            let apiKey = null;
            if (state.keyClear)
                apiKey = {action: 'clear'};
            else if (state.keyText.trim() !== '')
                apiKey = {action: 'set', value: state.keyText};
            return {fields, apiKey};
        }
        function snapshotValue(id) {
            return knownValues[id] ?? '';
        }

        // Per-card Apply: sends ONLY this card's mutation over stdin.
        const applyBtn = new Gtk.Button({label: _('Apply'), valign: Gtk.Align.CENTER});
        applyBtn.add_css_class('suggested-action');
        applyBtn.connect('clicked', () => {
            if (this._applyBusy)
                return;
            const {fields, apiKey} = collect();
            const fieldCount = Object.keys(fields).length;
            if (fieldCount === 0 && !apiKey) {
                showError(_('Nothing to apply — this card has no changes.'));
                return;
            }
            if (card.vendor === 'zai') {
                const type = fields.account_type !== undefined
                    ? fields.account_type : snapshotValue('account_type');
                if (type === 'team') {
                    const org = fields.organization_id !== undefined
                        ? fields.organization_id : snapshotValue('organization_id');
                    const proj = fields.project_id !== undefined
                        ? fields.project_id : snapshotValue('project_id');
                    if (String(org).trim() === '' || String(proj).trim() === '') {
                        showError(teamIdsRemedy());
                        return;
                    }
                }
            }
            const mutation = {action: 'update', vendor: card.vendor, label: card.label};
            if (fieldCount > 0)
                mutation.fields = fields;
            if (apiKey)
                mutation.apiKey = apiKey;
            this._sendMutation([mutation], showError);
        });
        row.add_action(applyBtn);
        this._applyButtons.push(applyBtn);

        const removeBtn = new Gtk.Button({label: _('Remove'), valign: Gtk.Align.CENTER});
        removeBtn.connect('clicked', () => {
            if (this._applyBusy)
                return;
            this._confirmRemove(card, showError);
        });
        row.add_action(removeBtn);

        syncTeamState();
        return row;
    }

    // A pending "Add account" draft: same fields as a card, Apply sends the
    // add mutation. Draft content lives in this row only — it is dropped by
    // the post-apply reload, like the QML form's scrub-on-save.
    _draftCard(entry) {
        const row = new Adw.ExpanderRow({
            title: _('New %s account').format(entry.display),
            expanded: true,
        });
        const state = {type: 'personal', org: '', proj: '', limit: '', key: ''};
        const supportsLimit = entry.cards.some(
            card => card.fields.some(field => field.id === 'monthly_limit'));
        // The vendor's own snapshot carries the account-type choices/labels
        // (Rust owns them); any card of the vendor has the field.
        const typeField = entry.vendor === 'zai'
            ? entry.cards.map(c => c.fields.find(f => f.id === 'account_type')).find(Boolean)
            : null;

        let orgEntry = null;
        let projEntry = null;
        const remedyRow = new Adw.ActionRow({title: teamIdsRemedy(), visible: false, title_lines: 3});
        remedyRow.add_css_class('error');
        const errorRow = new Adw.ActionRow({visible: false, title_lines: 3});
        errorRow.add_css_class('error');
        const showError = text => {
            errorRow.title = text;
            errorRow.visible = text !== '';
        };

        function syncTeamState() {
            const team = entry.vendor === 'zai' && state.type === 'team';
            if (orgEntry)
                orgEntry.visible = team;
            if (projEntry)
                projEntry.visible = team;
            remedyRow.visible = team && (state.org.trim() === '' || state.proj.trim() === '');
        }

        if (typeField) {
            const values = typeField.choices.length > 0 ? typeField.choices : ['personal'];
            const combo = new Adw.ComboRow({
                title: typeField.label,
                model: Gtk.StringList.new(values.map(choice => choiceLabel(typeField, choice))),
            });
            combo.connect('notify::selected', () => {
                state.type = values[combo.selected] ?? values[0];
                syncTeamState();
            });
            combo.selected = Math.max(0, values.indexOf(state.type));
            row.add_row(combo);
            orgEntry = new Adw.EntryRow({title: _('Organization ID')});
            orgEntry.connect('notify::text', () => {
                state.org = String(orgEntry.text ?? '');
                syncTeamState();
            });
            projEntry = new Adw.EntryRow({title: _('Project ID')});
            projEntry.connect('notify::text', () => {
                state.proj = String(projEntry.text ?? '');
                syncTeamState();
            });
            row.add_row(orgEntry);
            row.add_row(projEntry);
        }
        if (supportsLimit) {
            const limitEntry = new Adw.EntryRow({title: _('Monthly limit (USD, optional)')});
            limitEntry.connect('notify::text', () => {
                state.limit = String(limitEntry.text ?? '');
            });
            row.add_row(limitEntry);
        }
        const keyEntry = new Adw.PasswordEntryRow({
            title: _('Paste API key'),
        });
        if (typeof keyEntry.show_peek_icon === 'boolean')
            keyEntry.show_peek_icon = true;
        keyEntry.connect('notify::text', () => {
            state.key = String(keyEntry.text ?? '');
        });
        row.add_row(keyEntry);
        row.add_row(remedyRow);
        row.add_row(errorRow);

        const applyBtn = new Gtk.Button({label: _('Apply'), valign: Gtk.Align.CENTER});
        applyBtn.add_css_class('suggested-action');
        applyBtn.connect('clicked', () => {
            if (this._applyBusy)
                return;
            if (state.key.trim() === '' && entry.vendor !== 'zai') {
                showError(_('Enter an API key first.'));
                return;
            }
            if (entry.vendor === 'zai' && state.type === 'team'
                && (state.org.trim() === '' || state.proj.trim() === '')) {
                showError(teamIdsRemedy());
                return;
            }
            const fields = {};
            if (entry.vendor === 'zai') {
                fields.account_type = state.type;
                if (state.type === 'team') {
                    fields.organization_id = state.org;
                    fields.project_id = state.proj;
                }
            } else if (supportsLimit && state.limit.trim() !== '') {
                fields.monthly_limit = state.limit.trim();
            }
            const mutation = {action: 'add', vendor: entry.vendor};
            if (Object.keys(fields).length > 0)
                mutation.fields = fields;
            if (state.key.trim() !== '')
                mutation.apiKey = {action: 'set', value: state.key};
            this._sendMutation([mutation], showError);
        });
        row.add_action(applyBtn);
        this._applyButtons.push(applyBtn);

        const discardBtn = new Gtk.Button({label: _('Discard'), valign: Gtk.Align.CENTER});
        discardBtn.add_css_class('flat');
        discardBtn.connect('clicked', () => {
            if (this._applyBusy)
                return;
            const parent = row.get_parent();
            if (parent && parent.remove)
                parent.remove(row);
        });
        row.add_action(discardBtn);

        syncTeamState();
        return row;
    }

    _confirmRemove(card, showError) {
        const name = card.label === '' ? _('the default account') : card.display;
        const dialog = new Adw.MessageDialog({
            heading: _('Remove this account?'),
            body: _('%s · %s will be removed from the configuration file.').format(
                card.vendor_display, name),
            modal: true,
            transient_for: this._window,
        });
        dialog.add_button(_('Cancel'), 'cancel');
        dialog.add_button(_('Remove'), 'remove');
        dialog.default_response = 'cancel';
        dialog.set_response_appearance('remove', Adw.ResponseAppearance.DESTRUCTIVE);
        dialog.connect('response', (dlg, response) => {
            if (response === 'remove')
                this._sendMutation(
                    [{action: 'remove', vendor: card.vendor, label: card.label}], showError);
        });
        dialog.present();
    }

    // Build, send, and settle ONE mutation batch. Payloads travel over stdin
    // only — never argv, never the environment.
    _sendMutation(accountChanges, onError) {
        const built = buildSettingsPatch('', [], accountChanges);
        if (!built.ok) {
            onError(built.error);
            return;
        }
        this._setApplyBusy(true);
        runWithStdin([resolveBinary(this._settings), 'settings', 'apply'], built.payload,
            {timeoutSec: 30})
            .then(res => {
                if (this._closed)
                    return;
                if (res.ok && parseSettingsApplyResult(res.stdout)) {
                    // Refresh ping: the extension watches config-rev.
                    this._settings.set_int('config-rev',
                        this._settings.get_int('config-rev') + 1);
                    // Success: blank secrets + fresh snapshot, via a full
                    // reload and rebuild — the cheapest correct refresh.
                    this._reloadSnapshot();
                    return;
                }
                this._setApplyBusy(false);
                const message = this._applyFailureText(res);
                onError(message);
                // A binary that dies before reading the patch predates the
                // settings bridge: surface that verbatim and swap in the
                // terminal-settings fallback.
                if (message === 'binary exited before reading the patch')
                    this._showBanner(message);
            });
    }

    _applyFailureText(res) {
        if (res.error === 'binary exited before reading the patch'
            || res.stderr.includes('binary exited before reading the patch'))
            return 'binary exited before reading the patch';
        if (res.stderr.trim() !== '')
            return launchErrorMessage(res.exitCode, res.stderr);
        if (res.error !== '')
            return res.error;
        return _('The settings command did not confirm the save.');
    }

    // Late-arriving failure surface (e.g. a primary change that the binary
    // rejected). AdwPreferencesPage only appends, so lift the login group
    // out and re-add it: the banner lands right above it.
    _showBanner(message) {
        if (this._closed)
            return;
        const banner = this._bannerGroup(message);
        this._dynamic.push(banner);
        if (this._loginGroupRef) {
            this._page.remove(this._loginGroupRef);
            this._page.add(banner);
            this._page.add(this._loginGroupRef);
        } else {
            this._page.add(banner);
        }
    }

    // Single-flight: while any apply is in flight every Apply button and the
    // primary selector go insensitive, so no second mutation can race the
    // reload that follows a success.
    _setApplyBusy(busy) {
        this._applyBusy = busy;
        for (const btn of this._applyButtons)
            btn.sensitive = !busy;
        if (this._primaryRow)
            this._primaryRow.sensitive = !busy;
    }

    _loginGroup() {
        const group = new Adw.PreferencesGroup({
            title: _('Login flows'),
            description: _('OAuth vendors log in through their own CLI in a terminal; Antigravity needs its app, IDE, or an interactive agy session. Reopen this window to re-check the status.'),
        });
        for (const v of LOGIN_FLOWS) {
            const row = new Adw.ActionRow({title: v.name});
            const btn = new Gtk.Button({valign: Gtk.Align.CENTER});
            btn.add_css_class('flat');
            row.add_suffix(btn);

            const update = () => {
                if (v.kind === 'local') {
                    const productDetected = loginVendorConfigured(v);
                    row.subtitle = _('checking…');
                    this._checkCli(v.cli, installed => {
                        if (productDetected) {
                            row.subtitle = _('✓ Antigravity detected — keep the app, the IDE, or agy open');
                        } else if (installed) {
                            row.subtitle = _('agy installed — open a session to make the quota available');
                        } else {
                            row.subtitle = _('open or install the app, the IDE, or agy; there is no separate login');
                        }
                        btn.label = installed ? _('Open agy') : _('No separate login');
                        btn.sensitive = installed;
                    });
                    return;
                }
                btn.sensitive = true;
                if (loginVendorConfigured(v)) {
                    row.subtitle = _('✓ Configured');
                    btn.label = _('Log in again');
                    return;
                }
                row.subtitle = _('checking…');
                this._checkCli(v.cli, installed => {
                    row.subtitle = installed
                        ? `⚠ ${_('Not logged in')} — \`${v.login}\``
                        : `⚠ ${v.cli} ${_('not installed')} (${_('installs to ~/.local, no sudo')})`;
                    btn.label = installed ? _('Log in') : _('Install + log in');
                });
            };
            update();
            this._loginUpdates.push(update);

            btn.connect('clicked', () => {
                if (v.kind === 'oauth')
                    spawnInTerminal(oauthCommand(v));
                else
                    spawnArgvInTerminal([v.cli]);
                GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 4, () => {
                    if (!this._closed)
                        update();
                    return GLib.SOURCE_REMOVE;
                });
            });

            group.add(row);
        }
        return group;
    }

    // checkCliInstalled callback wrapped with the closed-window and
    // stale-render guards: a widget from an earlier render is never touched.
    _checkCli(cli, callback) {
        const gen = this._renderGen;
        checkCliInstalled(cli, result => {
            if (this._closed || gen !== this._renderGen)
                return;
            callback(result);
        });
    }
}
