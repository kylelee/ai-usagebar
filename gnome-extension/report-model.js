// Pure report/settings adapter for the GNOME Shell extension — the ESM port
// of omarchy/Model.js (the Quattro QML library, which has no exports and is
// loaded via `.pragma library`). Zero imports on purpose: no gi://, no node
// builtins, no omarchy/kde paths, so bare `node` can exercise the exact
// report contract and selection behavior in CI, exactly like the omarchy
// suite does. Normalization, caps, fallbacks, and string outputs are kept
// identical to Model.js.

export function cleanText(value, maxLength) {
    let text = value === undefined || value === null ? '' : String(value);
    // The Rust projection already strips terminal controls. This second, cheap
    // boundary keeps hand-authored/older JSON from putting controls in a
    // long-lived shell process.
    text = text.replace(/[\t\r]/g, ' ')
        .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g, '')
        .replace(/[\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, '');
    const limit = Number(maxLength) || 2048;
    if (text.length <= limit) return text;
    let end = limit - 1;
    const finalCodeUnit = text.charCodeAt(end - 1);
    if (finalCodeUnit >= 0xd800 && finalCodeUnit <= 0xdbff) end--;
    return text.slice(0, end) + '…';
}

// GNOME St labels are not rich text, but provider-controlled strings must
// never be reclassified as markup downstream either: replace angle brackets
// and fold line breaks so a hostile label stays one inert line of text.
export function autoTextSafe(value) {
    return cleanText(value, 1000)
        .replace(/[\n\u2028\u2029]/g, ' ')
        .replace(/</g, '‹')
        .replace(/>/g, '›');
}

export function finitePercent(value) {
    // `Number(null)` is 0 — a null percent (balance-row headline) would clamp
    // to 0 and read as 100% remaining. Absent means absent.
    if (value === null || value === undefined || value === '') return null;
    const number = Number(value);
    if (!isFinite(number)) return null;
    return Math.max(0, Math.min(100, Math.round(number)));
}

export function normalizeSection(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const type = String(raw.type || '');
    if (type === 'spacer') return {type: 'spacer'};
    if (type === 'metric') {
        const percent = finitePercent(raw.percent);
        if (percent === null) return null;
        let severity = String(raw.severity || '');
        if (!['low', 'mid', 'high', 'critical'].includes(severity))
            severity = percent >= 90 ? 'critical' : percent >= 75 ? 'high'
                : percent >= 50 ? 'mid' : 'low';
        const window = raw.window === 'session' || raw.window === 'weekly'
            ? raw.window : null;
        // The window's length in seconds (Rust-owned). With reset_at it feeds
        // the countdown figure's own clock band; null on older binaries and
        // rows without a window.
        const rawWindowSeconds = Number(raw.window_seconds);
        const windowSeconds = isFinite(rawWindowSeconds) && rawWindowSeconds > 0
            ? rawWindowSeconds : null;
        return {
            type: 'metric',
            label: cleanText(raw.label, 160),
            percent: percent,
            value: cleanText(raw.value, 240),
            detail: cleanText(raw.detail, 1000),
            severity: severity,
            reset_at: cleanText(raw.reset_at, 80),
            window: window,
            window_seconds: windowSeconds
        };
    }
    if (type === 'text') {
        return {
            type: 'text',
            label: cleanText(raw.label, 160),
            value: cleanText(raw.value, 1000)
        };
    }
    if (type === 'block') {
        const body = Array.isArray(raw.body) ? raw.body : [];
        const lines = [];
        for (let i = 0; i < body.length && i < 24; i++) lines.push(cleanText(body[i], 1000));
        return {type: 'block', label: cleanText(raw.label, 160), body: lines};
    }
    return null;
}

export function normalizeEntry(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const id = cleanText(raw.id, 180).trim();
    if (id === '') return null;
    const sourceSections = Array.isArray(raw.sections) ? raw.sections : [];
    const sections = [];
    for (let i = 0; i < sourceSections.length && i < 96; i++) {
        const section = normalizeSection(sourceSections[i]);
        if (section) sections.push(section);
    }
    const error = raw.error === undefined || raw.error === null ? '' : cleanText(raw.error, 1200);
    // "unconfigured" is the routine no-key/no-login state: same remedy text,
    // calmer presentation (no alarm color, no bar alert).
    let status = 'ready';
    if (error !== '' || raw.status === 'error')
        status = raw.unconfigured === true ? 'unconfigured' : 'error';
    return {
        id: id,
        name: cleanText(raw.name, 240),
        display_name: cleanText(raw.display_name, 240),
        short_name: cleanText(raw.short_name, 24),
        // Vendor logo asset id (`VendorId::logo_slug` in Rust). Empty for
        // binaries older than the field — the text tag stands in.
        logo: cleanText(raw.logo, 48),
        plan: cleanText(raw.plan, 240),
        status: status,
        error: error,
        unconfigured: raw.unconfigured === true,
        // Every window maxed / balance spent: the account has no capacity left
        // right now. Bar tiles hide; tabs and popups never do.
        exhausted: raw.exhausted === true,
        // Precomputed by the Rust core (report.rs `headline_pick`): the figure
        // a tile leads with. null for older binaries — headline() then walks
        // the sections locally, under the same rule.
        headline: normalizeHeadline(raw.headline),
        stale: raw.stale === true,
        fetched_at: cleanText(raw.fetched_at, 80),
        sections: sections
    };
}

export function normalizeHeadline(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const text = cleanText(raw.text, 240);
    if (text === '') return null;
    let severity = String(raw.severity || '');
    if (!['low', 'mid', 'high', 'critical'].includes(severity)) severity = 'low';
    return {
        text: text,
        percent: finitePercent(raw.percent),
        severity: severity,
        label: cleanText(raw.label, 160),
        reset_at: cleanText(raw.reset_at, 80)
    };
}

export function parseReport(raw) {
    try {
        const parsed = JSON.parse(String(raw || ''));
        if (!parsed || !Array.isArray(parsed.entries))
            return {ok: false, error: 'The usage command returned an unsupported report.', primary: '', entries: []};
        const entries = [];
        for (let i = 0; i < parsed.entries.length && i < 64; i++) {
            const entry = normalizeEntry(parsed.entries[i]);
            if (entry) entries.push(entry);
        }
        if (parsed.entries.length > 0 && entries.length === 0)
            return {ok: false, error: 'The usage report did not contain a valid provider entry.', primary: '', entries: []};
        return {ok: true, error: '', primary: cleanText(parsed.primary, 180).trim(), entries: entries};
    } catch {
        return {ok: false, error: 'The usage command returned invalid JSON.', primary: '', entries: []};
    }
}

export function baseProvider(id) {
    return String(id || '').split('@')[0];
}

export function providerName(entry) {
    if (!entry) return 'AI usage';
    // The Rust report owns canonical product names. `name` is the compatible
    // fallback for older binaries; the machine id is only a last resort.
    let title = cleanText(entry.display_name || entry.name, 240).trim();
    if (title === '') title = baseProvider(entry.id).replace(/_/g, ' ') || 'AI usage';
    return autoTextSafe(title);
}

export function filteredEntries(entries, configuredProvider) {
    const list = Array.isArray(entries) ? entries : [];
    const wanted = String(configuredProvider || '').trim().toLowerCase();
    if (wanted === '') return list.slice();
    const exact = list.filter(entry => String(entry.id).toLowerCase() === wanted);
    if (exact.length > 0) return exact;
    return list.filter(entry => baseProvider(entry.id).toLowerCase() === wanted);
}

// Panel tabs list only agents that actually read: unconfigured and broken
// entries stay reachable through the bar's wheel cycle and the status hint,
// but they get no tab.
export function readyEntries(entries) {
    const list = Array.isArray(entries) ? entries : [];
    return list.filter(entry => entry.status === 'ready');
}

// The dropdown's key tabs — ONE TAB PER KEY (account), tiled by key rather
// than by vendor: sorted by vendor first so a vendor's keys stay adjacent,
// then by key — the vendor default (no "@") leads, numbered siblings compare
// numerically (1 < 2 < 10), named keys compare as text. Only accounts that
// actually READ get a tab (the omarchy panel's readyEntries rule; exhausted
// ones keep theirs — tabs and popups never drop an account). Unconfigured
// and broken entries get no tab; a configured-but-broken account surfaces as
// a calm error row in extension.js instead. Tab labels are the account's own
// display_name via providerName — the report owns the names.
export function accountTabs(entries) {
    const tabs = readyEntries(entries);
    const accountKey = id => {
        const at = String(id || '').indexOf('@');
        return at >= 0 ? String(id).slice(at + 1) : '';
    };
    return tabs.slice().sort((a, b) => {
        const vendor = baseProvider(a.id).localeCompare(baseProvider(b.id), undefined,
            {sensitivity: 'base'});
        if (vendor !== 0) return vendor;
        const keyA = accountKey(a.id);
        const keyB = accountKey(b.id);
        if (keyA === '') return keyB === '' ? 0 : -1;
        if (keyB === '') return 1;
        const numericA = /^\d+$/.test(keyA) ? Number(keyA) : null;
        const numericB = /^\d+$/.test(keyB) ? Number(keyB) : null;
        if (numericA !== null && numericB !== null) return numericA - numericB;
        return keyA.localeCompare(keyB, undefined, {sensitivity: 'base'});
    });
}

// Bar tiles: READY accounts that still have capacity. An exhausted one (any
// window maxed, balance spent — the Rust report's `exhausted`) leaves the bar
// until its next report shows room again; recovery needs no state because the
// filter runs on every refresh. Tabs and popups keep the entry.
export function barEntries(entries) {
    const list = Array.isArray(entries) ? entries : [];
    return list.filter(entry => entry.status === 'ready' && entry.exhausted !== true);
}

// The tile population for the bar: all of `barEntries` when tiling is on (the
// default — the bar shows every working account side by side), or the SELECTED
// entry alone when tiling is off. The selected entry is skipped when it is
// exhausted — a maxed account is not a bar tile, tiling or not.
export function barTileEntries(entries, selectedId, tiled) {
    const working = barEntries(entries);
    if (tiled === false) {
        for (let i = 0; i < working.length; i++)
            if (working[i].id === selectedId) return [working[i]];
        return working.length > 0 ? [working[0]] : [];
    }
    return working;
}

export function selectedIndex(entries, selectedId) {
    const list = Array.isArray(entries) ? entries : [];
    for (let i = 0; i < list.length; i++) if (list[i].id === selectedId) return i;
    return list.length > 0 ? 0 : -1;
}

export function preferredEntryId(entries, primaryProvider, rememberedEntryId) {
    const list = Array.isArray(entries) ? entries : [];
    if (list.length === 0) return '';
    const byId = id => {
        const wanted = cleanText(id, 180).trim().toLowerCase();
        for (let k = 0; k < list.length; k++)
            if (String(list[k].id).toLowerCase() === wanted) return list[k];
        return null;
    };
    let entry = byId(rememberedEntryId);
    if (entry && entry.status === 'ready') return entry.id;
    const primary = String(primaryProvider || '').toLowerCase();
    entry = byId(primary);
    if (entry && entry.status === 'ready') return entry.id;
    for (let j = 0; j < list.length; j++)
        if (baseProvider(list[j].id).toLowerCase() === primary
            && list[j].status === 'ready') return list[j].id;
    // Nothing remembered or primary that reads: the first working account.
    for (let r = 0; r < list.length; r++)
        if (list[r].status === 'ready') return list[r].id;
    return list[0].id;
}

// The per-account tag of a bar tile. A named account shows its own name ONLY
// (kimi-main 37% — the provider stays in the popup); an unnamed default
// account shows the provider name (kimi 37%), taken from the report id the
// Rust side owns. Same rule the Rust `--vendor all` bar applies, so the two
// presentations can never disagree about a tile's name.
export function tileTag(entry) {
    if (!entry) return '';
    const id = String(entry.id || '');
    const at = id.indexOf('@');
    if (at >= 0 && at + 1 < id.length) return autoTextSafe(id.slice(at + 1)).trim();
    return autoTextSafe(baseProvider(entry.id)).trim();
}

// The four-step range class for ONE window's percent — the remaining bands
// every tile carries, so usage ranges read at a glance, not just alerts:
//   remaining < 5%  → "critical" (red)
//   remaining < 20% → "high"     (orange)
//   remaining < 50% → "mid"      (yellow)
//   otherwise       → "low"      (green)
export function remainingClass(percent) {
    if (percent === null || percent === undefined) return '';
    const remaining = 100 - Number(percent);
    if (remaining < 5) return 'critical';
    if (remaining < 20) return 'high';
    if (remaining < 50) return 'mid';
    return 'low';
}

// A window metric's own class — the per-figure color source, independent of
// every other window on the same account.
export function windowClass(metric) {
    if (!metric) return '';
    return remainingClass(metric.percent);
}

// The tile's color class: the least REMAINING across its shown windows (the
// worst band wins). Drives the no-window fallback figure; the tile's tag
// itself stays neutral by design.
export function tileRemainingClass(entry) {
    if (!entry || entry.status !== 'ready') return '';
    let rows = tileWindows(entry);
    if (rows.length === 0) {
        const sections = entry.sections || [];
        for (let i = 0; i < sections.length; i++)
            if (sections[i].type === 'metric') { rows.push(sections[i]); break; }
    }
    const rank = {low: 0, mid: 1, high: 2, critical: 3};
    let worst = '';
    for (let r = 0; r < rows.length; r++) {
        const cls = windowClass(rows[r]);
        if (cls === '') continue;
        if (worst === '' || rank[cls] > rank[worst]) worst = cls;
    }
    return worst;
}

// One-unit bar-tile countdown, mirroring Rust's `countdown::format_compact`:
//   under an hour  → 42m  (whole minutes, rounded up; capped at 59m)
//   under a day    → 5.3h (tenths of an hour; "5h" when the tenth is zero)
//   a day or more  → 2.3d (tenths of a day; "7d" for a full week)
// Truncated past the minutes bucket so the bar never claims more time than
// remains. Panels and popups keep `formatDuration`'s two-component detail.
export function compactReset(resetAt, nowMs) {
    if (!resetAt) return '';
    const resetMs = new Date(String(resetAt)).getTime();
    if (!isFinite(resetMs)) return '';
    const remaining = resetMs - Number(nowMs);
    if (remaining <= 0) return 'due';
    const seconds = Math.floor(remaining / 1000);
    if (seconds < 3600) return Math.min(59, Math.ceil(seconds / 60)) + 'm';
    const subDay = seconds < 86400;
    const unitSecs = subDay ? 3600 : 86400;
    const suffix = subDay ? 'h' : 'd';
    const tenths = Math.floor(seconds * 10 / unitSecs);
    const whole = Math.floor(tenths / 10);
    const frac = tenths % 10;
    return frac === 0 ? whole + suffix : whole + '.' + frac + suffix;
}

// The countdown figure's own class: what fraction of the window's TIME is
// still on the clock — 1h left of a 5h window reads as 20%, 1.1d of 7d as
// 15.7% — banded by the same remaining scale the percentage figures use, so
// the clock's color escalates on its own, independent of the usage percent
// beside it. `remainingClass` expects a utilization percent; the fraction
// here is itself the REMAINING side, so it goes in as the complement.
export function countdownClass(resetAt, windowSeconds, nowMs) {
    if (!resetAt || !windowSeconds) return '';
    const resetMs = new Date(String(resetAt)).getTime();
    if (!isFinite(resetMs)) return '';
    const secs = (resetMs - Number(nowMs)) / 1000;
    if (!isFinite(secs)) return '';
    return remainingClass(100 - secs / Number(windowSeconds) * 100);
}

// The two windows a tile leads with: the rolling ~5h session and the weekly
// quota. The classification is Rust-owned (`window` on each metric row);
// this only picks the rows, so the rule can never fork between frontends.
function tileWindows(entry) {
    let session = null, weekly = null;
    const sections = entry && entry.sections ? entry.sections : [];
    for (let i = 0; i < sections.length; i++) {
        const section = sections[i];
        if (section.type !== 'metric') continue;
        if (!session && section.window === 'session') session = section;
        else if (!weekly && section.window === 'weekly') weekly = section;
    }
    const out = [];
    if (session) out.push(session);
    if (weekly) out.push(weekly);
    return out;
}

// How many entries share each provider. An account's suffix exists to
// distinguish it from its SIBLINGS — when a provider has only one key, the
// suffix is pure noise ("zai@1" has nothing to be distinguished from), so the
// tile degrades to exactly what a default account shows: the logo alone / the
// provider name. Counted over the whole visible report, exhausted and broken
// siblings included: a hidden second key still exists, and when it recovers
// both tiles get their suffixes back.
export function providerEntryCounts(entries) {
    const counts = {};
    const list = Array.isArray(entries) ? entries : [];
    for (let i = 0; i < list.length; i++) {
        const base = baseProvider(list[i].id);
        counts[base] = (counts[base] || 0) + 1;
    }
    return counts;
}

// One account's tile as separable parts: the tag first (NEUTRAL — the tag
// keeps the theme foreground color, never a usage range), then per shown
// window TWO parts carrying their OWN classes — the percentage and its
// countdown render as independent spans with independent colors: a green 5h
// figure ("99% left") can sit beside an orange weekly one ("13% left"), and
// each countdown colors by how much of the window's CLOCK is left rather
// than by its usage percent. Critical-bold is NOT baked in: `cls ===
// "critical"` is consumed at the render site.
//
// The tag part also carries the vendor logo asset id (`logo`). Beside a
// rendered logo only a NAMED account still needs its distinguishing text
// (`logoLabel` — the account suffix); a default account is the logo alone.
// `solo === true` (the provider's only entry) extends that to named accounts:
// no suffix, and the text fallback becomes the provider name. `text` stays the
// full tag so a frontend without the asset (or a binary older than the field)
// falls back to exactly the old tile. `showRemaining` flips percentages to
// what is LEFT of the window; `showValue` off degrades every tile to its tag.
// Balance-style accounts (no windows) keep the single headline figure.
export function tileParts(entry, showValue, showRemaining, nowMs, solo) {
    if (!entry) return [];
    const tag = tileTag(entry);
    const logo = entry && entry.logo ? cleanText(entry.logo, 48) : '';
    const id = String((entry && entry.id) || '');
    const at = id.indexOf('@');
    const logoLabel = at >= 0 && at + 1 < id.length
        ? autoTextSafe(id.slice(at + 1)).trim() : '';
    let tagPart;
    if (solo === true) {
        // The provider's only key: the logo/name identifies it completely.
        const base = autoTextSafe(baseProvider(entry.id)).trim();
        tagPart = tag === '' ? null : {text: base, cls: '', logo: logo, logoLabel: ''};
    } else {
        tagPart = tag === '' ? null
            : {text: tag, cls: '', logo: logo, logoLabel: logoLabel};
    }
    if (!showValue) return tagPart ? [tagPart] : [];
    const windows = tileWindows(entry);
    const parts = [];
    if (windows.length > 0) {
        if (tagPart) parts.push(tagPart);
        for (let i = 0; i < windows.length; i++) {
            const metric = windows[i];
            const pct = showRemaining ? 100 - metric.percent : metric.percent;
            // A window without a countdown (not started yet) keeps its slot
            // with a dash placeholder — `0%↺-` lines up with `38%↺2h` instead
            // of looking like the timer failed to load. The ↺ glyph marks the
            // figure as the reset countdown; `glue` keeps it flush against
            // its percent (no space at the render site).
            let reset = compactReset(metric.reset_at, nowMs);
            if (reset === '') reset = '-';
            parts.push({text: pct + '%', cls: windowClass(metric)});
            parts.push({
                text: '↺' + reset,
                cls: countdownClass(metric.reset_at, metric.window_seconds, nowMs),
                glue: true
            });
        }
        return parts;
    }
    // No windows (balance-style accounts): the single headline figure.
    const summary = headline(entry);
    let text = autoTextSafe(summary.text).trim();
    if (summary.percent !== null && showRemaining) text = (100 - summary.percent) + '%';
    if (text === '' || text === 'Ready' || text === 'Error')
        return tagPart ? [tagPart] : [];
    if (tagPart) parts.push(tagPart);
    parts.push({text: text, cls: tileRemainingClass(entry)});
    return parts;
}

export function headline(entry) {
    if (!entry) return {text: '', percent: null, severity: 'low', label: '', reset_at: ''};
    // The Rust core precomputes the pick (report.rs `headline_pick`); trust it
    // when present. Older binaries fall through to the local walk below,
    // which implements the same rule.
    if (entry.headline) {
        const pre = entry.headline;
        return {
            text: pre.text,
            percent: pre.percent,
            severity: pre.severity,
            label: pre.label,
            reset_at: pre.reset_at
        };
    }
    let best = null;
    const sections = entry.sections || [];
    for (let i = 0; i < sections.length; i++) {
        const section = sections[i];
        if (section.type === 'metric' && (!best || section.percent > best.percent)) best = section;
    }
    if (best) {
        const bestText = /balance/i.test(best.label) && best.value !== ''
            ? best.value : best.percent + '%';
        return {
            text: bestText,
            percent: best.percent,
            severity: best.severity,
            label: best.label,
            reset_at: best.reset_at || ''
        };
    }
    for (let j = 0; j < sections.length; j++) {
        const row = sections[j];
        if (row.type === 'text' && /(balance|available|spend|prepaid)/i.test(row.label) && row.value !== '')
            return {text: row.value, percent: null, severity: 'low', label: row.label, reset_at: ''};
    }
    return {text: entry.status === 'error' ? 'Error' : 'Ready', percent: null, severity: 'low', label: '', reset_at: ''};
}

export function isAlarming(entry) {
    if (!entry) return false;
    if (entry.status === 'unconfigured') return false;
    const summary = headline(entry);
    return entry.status === 'error' || entry.stale === true || summary.severity === 'critical';
}

// The alarm class ONE bar tile's neutral text carries: only its OWN
// account's urgent state (error, stale, critical headline) — never another
// vendor's. Before this, any critical account (kimi's maxed 5h window)
// reddened EVERY tile's tag and every balance figure through .aiub-alarm;
// isolation keeps zai's suffix, minimax's tag, and deepseek's money figure
// in their own colors while the critical account still alarms itself.
export function tileAlarmClass(entry) {
    return isAlarming(entry) ? 'aiub-alarm' : '';
}

// The robot (single-account) label's alarm: bar-wide failures — a refresh
// error or a vendor filter matching nothing — or the SHOWN account's own
// urgency. Other accounts' critical headlines never reach it.
export function robotAlarmClass(reportError, filterMiss, shown) {
    if (reportError !== '' || filterMiss)
        return 'aiub-alarm';
    return tileAlarmClass(shown);
}

export function formatDuration(milliseconds) {
    if (!(milliseconds > 0)) return 'now';
    const minutes = Math.floor(milliseconds / 60000);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);
    if (days > 0) return days + 'd ' + (hours % 24) + 'h';
    if (hours > 0) return hours + 'h ' + (minutes % 60) + 'm';
    return Math.max(1, minutes) + 'm';
}

const MONTH_NAMES = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function pad2(value) {
    return ('0' + value).slice(-2);
}

function isSameLocalDay(a, b) {
    return a.getFullYear() === b.getFullYear()
        && a.getMonth() === b.getMonth()
        && a.getDate() === b.getDate();
}

export function formatReset(resetAt, nowMs) {
    if (!resetAt) return '';
    const resetMs = new Date(String(resetAt)).getTime();
    if (!isFinite(resetMs)) return '';
    const remaining = resetMs - Number(nowMs);
    if (remaining <= 0) return 'Reset due';
    // Show the real local clock time the window reopens, so the absolute reset
    // moment is visible next to the countdown. Date it whenever it lands on
    // another day: a bare "03:00" on an 18h countdown reads as a time that has
    // already passed, which is the ambiguity this row exists to remove. Keyed
    // on the calendar day rather than on "is it 24h away", because tonight's
    // reset crosses midnight long before it crosses 24h.
    const at = new Date(resetMs);
    let clock = pad2(at.getHours()) + ':' + pad2(at.getMinutes());
    if (!isSameLocalDay(at, new Date(Number(nowMs))))
        clock = MONTH_NAMES[at.getMonth()] + ' ' + at.getDate() + ' ' + clock;
    return 'Resets in ' + formatDuration(remaining) + ' · ' + clock;
}

export function formatUpdated(fetchedAt, nowMs) {
    if (!fetchedAt) return 'Updated time unavailable';
    const fetchedMs = new Date(String(fetchedAt)).getTime();
    if (!isFinite(fetchedMs)) return 'Updated time unavailable';
    const elapsed = Math.max(0, Number(nowMs) - fetchedMs);
    if (elapsed < 60000) return 'Updated just now';
    return 'Updated ' + formatDuration(elapsed) + ' ago';
}

export function metricDetail(row) {
    let detail = cleanText(row && row.detail, 1000);
    if (!row || !row.reset_at) return detail;
    // Older human-readable reset text remains useful to CLI consumers. Strip
    // just that fragment in the native panel, which renders a live countdown.
    detail = detail.replace(/^Resets in [^·]+\s*(?:·\s*)?/i, '');
    detail = detail.replace(/\s*·\s*reset\s+[^·]+$/i, '');
    return detail.trim();
}

export function errorMessage(value) {
    const message = cleanText(value, 500).trim();
    return message === '' ? 'The usage command failed without an error message.' : message;
}

// The extension launches ai-usagebar-omarchy through /usr/bin/env, so a
// missing binary comes back as exit 127 instead of the process simply never
// starting — surface that as the install remedy instead of the helper's raw
// "not found" text or a widget silently stuck on its loading state.
export function launchErrorMessage(exitCode, stderrText) {
    if (Number(exitCode) === 127)
        return 'ai-usagebar-omarchy is not installed. The plugin is only the display frontend. Install Rust from https://rustup.rs if needed, then: cargo install --git https://github.com/KyleLee/ai-usagebar-omarchy';
    return errorMessage(stderrText);
}

// Bundled logo assets — one file per asset id under gnome-extension/logos/.
// Rust owns the vendor → id mapping (`VendorId::logo_slug`, shipped as each
// entry's `logo`); this table only records which files actually ship, so a
// missing or future asset degrades to the text tag instead of a broken image
// in a long-lived shell process. Keys mirror the Rust slugs; keep both in
// lockstep (the contract test walks every vendor id against it).
export const LOGO_ASSETS = Object.freeze({
    claude: 'claude.svg',
    anthropic: 'anthropic.svg',
    openai: 'openai.png',
    zai: 'zai.png',
    openrouter: 'openrouter.svg',
    deepseek: 'deepseek.svg',
    kimi: 'kimi.svg',
    kilo: 'kilo.png',
    novita: 'novita.png',
    moonshot: 'moonshot.png',
    grok: 'grok.png',
    antigravity: 'antigravity.svg',
    cursor: 'cursor.svg',
    minimax: 'minimax.svg',
    kiro: 'kiro.png',
    opencode: 'opencode.svg'
});

// The bundled file name for a logo id, or '' when no asset ships.
export function logoAssetName(slug) {
    const id = cleanText(slug, 48);
    return Object.prototype.hasOwnProperty.call(LOGO_ASSETS, id)
        ? LOGO_ASSETS[id] : '';
}

export function settingsId(value) {
    const id = cleanText(value, 80).trim();
    if (!/^[a-z0-9_-]+$/.test(id)
        || id === '__proto__' || id === 'constructor' || id === 'prototype') return '';
    return id;
}

// The Rust bridge deliberately returns only key-presence booleans. Keep this
// parser strict so a compromised/older helper cannot smuggle rich text or an
// unbounded model into the long-lived shell process.
// Field ids share the vendor-id alphabet (settingsId already allows _).
export function fieldId(value) {
    return settingsId(value);
}

// A per-vendor editable field on an account card. `text` renders a line edit,
// `choice` a dropdown (with optional display labels — the canonical product
// names live in Rust), `secret` a password row whose value never travels
// (presence rides the account's configured flags).
export function normalizeField(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const id = fieldId(raw.id);
    if (id === '') return null;
    const kind = raw.kind === 'choice' ? 'choice' : raw.kind === 'secret' ? 'secret' : 'text';
    const choices = [];
    if (kind === 'choice' && Array.isArray(raw.choices)) {
        for (let i = 0; i < raw.choices.length && i < 12; i++) {
            const choice = cleanText(raw.choices[i], 40);
            if (choice !== '') choices.push(choice);
        }
    }
    const labels = {};
    if (raw.labels && typeof raw.labels === 'object' && !Array.isArray(raw.labels)) {
        for (const key in raw.labels) {
            if (key === '__proto__' || key === 'constructor' || key === 'prototype') continue;
            const value = cleanText(raw.labels[key], 60);
            if (value !== '') labels[key] = value;
        }
    }
    return {
        id: id,
        label: cleanText(raw.label, 120) || id,
        kind: kind,
        value: cleanText(raw.value, 200),
        choices: choices,
        labels: labels
    };
}

// One account of a multi-account vendor (every key-based provider): a labeled
// bundle of fields plus key-presence booleans. label '' is the vendor's
// default section. `vendor_display` is Rust's canonical provider name — the
// settings form's group titles and Add buttons render it.
export function normalizeAccount(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const vendor = settingsId(raw.vendor);
    const label = cleanText(raw.label, 120);
    if (vendor === '') return null;
    const fields = [];
    const source = Array.isArray(raw.fields) ? raw.fields : [];
    for (let i = 0; i < source.length && i < 16; i++) {
        const field = normalizeField(source[i]);
        if (field) fields.push(field);
    }
    return {
        vendor: vendor,
        vendor_display: cleanText(raw.vendor_display, 120) || vendor,
        label: label,
        display: cleanText(raw.display, 120) || (label === '' ? 'Default' : label),
        environment: cleanText(raw.environment, 160),
        configured: raw.configured === true,
        inline_configured: raw.inline_configured === true,
        environment_configured: raw.environment_configured === true,
        fields: fields
    };
}

export function parseSettingsSnapshot(raw) {
    try {
        const parsed = JSON.parse(String(raw || ''));
        if (!parsed || Number(parsed.schema_version) !== 1
            || !Array.isArray(parsed.primary_choices) || !Array.isArray(parsed.keys))
            return {ok: false, error: 'The settings command returned an unsupported response.', primary: '', primary_choices: [], keys: [], accounts: []};

        const choices = [];
        for (let i = 0; i < parsed.primary_choices.length && i < 64; i++) {
            const choice = parsed.primary_choices[i];
            const choiceId = settingsId(choice && choice.id);
            if (choiceId === '') continue;
            choices.push({id: choiceId, value: choiceId, label: cleanText(choice.label, 120) || choiceId});
        }

        const keys = [];
        for (let j = 0; j < parsed.keys.length && j < 32; j++) {
            const key = parsed.keys[j];
            const keyId = settingsId(key && key.id);
            if (keyId === '') continue;
            const fields = [];
            const rawFields = key && Array.isArray(key.fields) ? key.fields : [];
            for (let f = 0; f < rawFields.length && f < 12; f++) {
                const field = normalizeField(rawFields[f]);
                if (field) fields.push(field);
            }
            keys.push({
                id: keyId,
                label: cleanText(key.label, 120) || keyId,
                environment: cleanText(key.environment, 160),
                note: cleanText(key.note, 240),
                configured: key.configured === true,
                inline_configured: key.inline_configured === true,
                environment_configured: key.environment_configured === true,
                fields: fields
            });
        }

        let primary = settingsId(parsed.primary);
        let primaryAvailable = false;
        for (let k = 0; k < choices.length; k++) {
            if (choices[k].id === primary) {
                primaryAvailable = true;
                break;
            }
        }
        const accounts = [];
        const rawAccounts = Array.isArray(parsed.accounts) ? parsed.accounts : [];
        for (let a = 0; a < rawAccounts.length && a < 16; a++) {
            const account = normalizeAccount(rawAccounts[a]);
            if (account) accounts.push(account);
        }

        if (!primaryAvailable) primary = choices.length > 0 ? choices[0].id : '';
        return {
            ok: true, error: '', primary: primary,
            primary_choices: choices, keys: keys, accounts: accounts
        };
    } catch {
        return {ok: false, error: 'The settings command returned invalid JSON.', primary: '', primary_choices: [], keys: [], accounts: []};
    }
}

// accountChanges: [{action:"update",vendor,label,fields,apiKey:{action,value}} |
//                  {action:"add",vendor,name,fields,apiKey} |
//                  {action:"remove",vendor,label}]
// Never throws for bad input — every rejection is an ok:false result, so a
// hostile or stale caller cannot crash the long-lived shell process.
export function buildSettingsPatch(primary, changes, accountChanges) {
    const primaryId = settingsId(primary);
    const rawPrimary = String(primary || '').trim();
    if (rawPrimary !== '' && primaryId === '')
        return {ok: false, error: 'Choose a valid primary provider.', payload: ''};
    const keys = {};
    const list = Array.isArray(changes) ? changes : [];
    const seen = [];
    for (let i = 0; i < list.length; i++) {
        const change = list[i] || {};
        const id = settingsId(change.id);
        if (id === '' || seen.includes(id))
            return {ok: false, error: 'A settings row has an invalid provider id.', payload: ''};
        seen.push(id);
        let fields = null;
        if (change.fields && typeof change.fields === 'object'
            && !Array.isArray(change.fields)) {
            fields = {};
            for (const fieldKey in change.fields) {
                if (fieldKey === '__proto__' || fieldKey === 'constructor' || fieldKey === 'prototype')
                    continue;
                const fid = fieldId(fieldKey);
                const value = String(change.fields[fieldKey] || '');
                if (fid === '')
                    return {ok: false, error: 'A settings field has an invalid id.', payload: ''};
                if (value.length > 200)
                    return {ok: false, error: 'A settings field is too long.', payload: ''};
                fields[fid] = value;
            }
            if (Object.keys(fields).length === 0) fields = null;
        }
        if (change.action === 'clear') {
            keys[id] = fields ? {action: 'clear', fields: fields} : {action: 'clear'};
        } else if (change.action === 'set') {
            const value = String(change.value || '').trim();
            if (value === '') return {ok: false, error: 'An edited API key is empty.', payload: ''};
            if (value.length > 16384) return {ok: false, error: 'An API key is too long.', payload: ''};
            keys[id] = fields ? {action: 'set', value: value, fields: fields}
                : {action: 'set', value: value};
        } else if (change.action === 'fields') {
            if (!fields)
                return {ok: false, error: 'A field-only change has no fields.', payload: ''};
            keys[id] = {action: 'fields', fields: fields};
        } else return {ok: false, error: 'A settings row has an invalid action.', payload: ''};
    }
    const accountsByVendor = {};
    const accountList = Array.isArray(accountChanges) ? accountChanges : [];
    for (let n = 0; n < accountList.length; n++) {
        const change = accountList[n] || {};
        const vendor = settingsId(change.vendor);
        if (vendor === '')
            return {ok: false, error: 'An account change has an invalid vendor.', payload: ''};
        const mutation = {action: change.action};
        if (change.action === 'update' || change.action === 'remove') {
            if (typeof change.label !== 'string')
                return {ok: false, error: 'An account change is missing its label.', payload: ''};
            mutation.label = cleanText(change.label, 120);
        } else if (change.action === 'add') {
            // The form no longer asks for a name: an omitted name means the
            // server auto-names the account (Z.AI, Z.AI 2 …). An explicit one
            // is still accepted for older callers.
            const name = String(change.name || '').trim();
            if (name !== '') {
                if (name.length > 120)
                    return {ok: false, error: 'An account name is too long.', payload: ''};
                mutation.name = name;
            }
        } else {
            return {ok: false, error: 'An account change has an invalid action.', payload: ''};
        }
        if (change.action !== 'remove') {
            if (change.fields && typeof change.fields === 'object' && !Array.isArray(change.fields)) {
                const fields = {};
                for (const fk in change.fields) {
                    if (fk === '__proto__' || fk === 'constructor' || fk === 'prototype') continue;
                    const fid = fieldId(fk);
                    // Trimmed: pasted ids/names carry stray whitespace far more
                    // often than any of these fields can meaningfully contain
                    // it. (Key-section fields above deliberately keep spaces —
                    // blank-vs-clear and exact-value semantics live there.)
                    const fvalue = String(change.fields[fk] || '').trim();
                    if (fid === '')
                        return {ok: false, error: 'An account field has an invalid id.', payload: ''};
                    if (fvalue.length > 200)
                        return {ok: false, error: 'An account field is too long.', payload: ''};
                    fields[fid] = fvalue;
                }
                if (Object.keys(fields).length > 0) mutation.fields = fields;
            }
            if (change.apiKey && typeof change.apiKey === 'object') {
                if (change.apiKey.action === 'set') {
                    const keyValue = String(change.apiKey.value || '').trim();
                    if (keyValue === '')
                        return {ok: false, error: 'An edited account API key is empty.', payload: ''};
                    if (keyValue.length > 16384)
                        return {ok: false, error: 'An account API key is too long.', payload: ''};
                    mutation.api_key = {action: 'set', value: keyValue};
                } else if (change.apiKey.action === 'clear') {
                    mutation.api_key = {action: 'clear'};
                }
            }
        }
        if (!accountsByVendor[vendor]) accountsByVendor[vendor] = [];
        accountsByVendor[vendor].push(mutation);
    }

    if (primaryId === '' && seen.length === 0 && Object.keys(accountsByVendor).length === 0)
        return {ok: false, error: 'There are no settings changes to save.', payload: ''};
    const patch = {schema_version: 1, keys: keys};
    if (Object.keys(accountsByVendor).length > 0) patch.accounts = accountsByVendor;
    if (primaryId !== '') patch.primary = primaryId;
    return {
        ok: true,
        error: '',
        payload: JSON.stringify(patch)
    };
}

export function parseSettingsApplyResult(raw) {
    try {
        const parsed = JSON.parse(String(raw || ''));
        return parsed && parsed.ok === true;
    } catch {
        return false;
    }
}

// ── Vendor dropdown data ───────────────────────────────────────────────────
// Canonical VendorId slugs mirroring src/vendor.rs's enum order exactly.
// Static completeness is deliberate: the report only lists ENABLED vendors,
// and a vendor the user wants to enable must stay selectable — slugs only,
// never display names (those come from the report; this is the one
// documented exception to the no-provider-tables rule).
export const VENDOR_SLUGS = Object.freeze([
    'anthropic', 'anthropic_api', 'openai', 'openai_api', 'zai', 'openrouter',
    'deepseek', 'kimi', 'kilo', 'novita', 'moonshot', 'grok', 'supergrok',
    'antigravity', 'cursor', 'minimax', 'kiro', 'opencode-go',
]);

