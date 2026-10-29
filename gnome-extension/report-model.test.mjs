// Contract suite for gnome-extension/report-model.js — the ESM port of
// omarchy/Model.js that extension.js and prefs.js consume. Bare node, no npm
// deps, no framework: `node gnome-extension/report-model.test.mjs`.
//
// Every case asserts REAL observable behavior with concrete fixtures and
// literal expected strings — nothing re-derives an expected value by calling
// the implementation. Structural gates pin wiring invariants at the
// source-text level (node cannot import gi:// modules) and are grouped at
// the bottom, one block per frontend file.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {
    LOGO_ASSETS, VENDOR_SLUGS, accountTabs, buildSettingsPatch, compactReset, countdownClass,
    headline, parseReport, parseSettingsSnapshot, providerName, remainingClass,
    robotAlarmClass, tileAlarmClass, tileParts
} from './report-model.js';

// One clearly named block per contract area — new blocks append to the
// list, never reorganize it. A failed group prints its named case and fails the
// run; the rest keep going so one mutation surfaces every broken edge.
const summary = [];
let failures = 0;
function runGroup(name, body) {
    try {
        body();
        summary.push('pass  ' + name);
    } catch (error) {
        failures++;
        summary.push('FAIL  ' + name);
        console.error('FAIL [' + name + '] ' + (error && error.message ? error.message : error));
        if (error && error.stack) console.error(error.stack.split('\n').slice(1, 4).join('\n'));
    }
}

// Fixed clock for every countdown: compactReset is pure UTC arithmetic, so a
// literal nowMs keeps the suite wall-clock independent.
const NOW = Date.parse('2026-09-02T12:00:00Z');

// ── parseReport ───────────────────────────────────────────────────────────
runGroup('parseReport: valid multi-entry report', () => {
    const raw = JSON.stringify({primary: 'openai', entries: [
        {
            id: 'anthropic@work', name: 'anthropic · work', display_name: 'Claude · work',
            short_name: 'cld', logo: 'claude', plan: 'Claude Max 20x', status: 'ready',
            error: null, stale: true, fetched_at: '2026-08-14T12:00:00Z',
            headline: {text: '29%', percent: 29, severity: 'low', label: 'Session (5h)',
                reset_at: '2026-08-14T14:00:00Z'},
            sections: [
                {type: 'spacer'},
                {type: 'metric', label: 'Session (5h)', percent: 29, value: '29%',
                    detail: 'Resets in 2h 0m · 60% elapsed · 31pts under', severity: 'low',
                    reset_at: '2026-08-14T14:00:00Z', window: 'session'},
                {type: 'text', label: 'Balance', value: '$12.00'},
                {type: 'block', label: 'Credits', body: ['balance: 20', '≈ 10 messages']}
            ]
        },
        {
            id: 'openai', name: 'openai', display_name: 'Codex', short_name: 'gpt',
            plan: 'Plus', error: null,
            sections: [{type: 'metric', label: 'Codex weekly', percent: 95, value: '95%',
                detail: '', severity: 'critical', window: 'weekly'}]
        }
    ]});
    const parsed = parseReport(raw);
    assert.equal(parsed.ok, true, 'valid report parses ok');
    assert.equal(parsed.error, '');
    assert.equal(parsed.primary, 'openai');
    assert.equal(parsed.entries.length, 2);
    const named = parsed.entries[0];
    assert.equal(named.id, 'anthropic@work', 'named account keeps its full id');
    assert.equal(named.name, 'anthropic · work');
    assert.equal(named.display_name, 'Claude · work');
    assert.equal(named.short_name, 'cld');
    assert.equal(named.logo, 'claude');
    assert.equal(named.plan, 'Claude Max 20x');
    assert.equal(named.status, 'ready');
    assert.equal(named.stale, true);
    assert.equal(named.fetched_at, '2026-08-14T12:00:00Z');
    assert.deepEqual(named.headline, {text: '29%', percent: 29, severity: 'low',
        label: 'Session (5h)', reset_at: '2026-08-14T14:00:00Z'});
    // Section order is the report's display order: spacer, metric, text, block.
    assert.deepEqual(named.sections.map(section => section.type),
        ['spacer', 'metric', 'text', 'block']);
    assert.deepEqual(named.sections[0], {type: 'spacer'});
    assert.deepEqual(named.sections[1], {type: 'metric', label: 'Session (5h)', percent: 29,
        value: '29%', detail: 'Resets in 2h 0m · 60% elapsed · 31pts under', severity: 'low',
        reset_at: '2026-08-14T14:00:00Z', window: 'session', window_seconds: null});
    assert.deepEqual(named.sections[2], {type: 'text', label: 'Balance', value: '$12.00'});
    assert.deepEqual(named.sections[3], {type: 'block', label: 'Credits',
        body: ['balance: 20', '≈ 10 messages']});
    const second = parsed.entries[1];
    assert.equal(second.id, 'openai');
    assert.equal(second.sections[0].severity, 'critical');
    assert.equal(second.sections[0].window, 'weekly');
});

runGroup('parseReport: malformed JSON and wrong shapes never throw', () => {
    const cases = [
        ['{oops', 'The usage command returned invalid JSON.'],
        ['', 'The usage command returned invalid JSON.'],
        ['null', 'The usage command returned an unsupported report.'],
        ['[]', 'The usage command returned an unsupported report.'],
        ['{}', 'The usage command returned an unsupported report.'],
        ['42', 'The usage command returned an unsupported report.'],
        ['{"entries":"nope"}', 'The usage command returned an unsupported report.']
    ];
    for (const [raw, expectedError] of cases) {
        const parsed = parseReport(raw);
        assert.equal(parsed.ok, false, 'input ' + JSON.stringify(raw) + ' is rejected');
        assert.equal(parsed.error, expectedError, 'input ' + JSON.stringify(raw));
        assert.deepEqual(parsed.entries, []);
        assert.equal(parsed.primary, '');
    }
    // Entries that all lack an id: normalization drops them, the report is
    // rejected as a whole rather than silently empty.
    const noIds = parseReport('{"entries":[{"name":"missing id"}]}');
    assert.equal(noIds.ok, false);
    assert.equal(noIds.error, 'The usage report did not contain a valid provider entry.');
});

runGroup('parseReport: older-binary tolerance and caps', () => {
    // A binary older than the logo/headline/exhausted fields still parses:
    // absent fields degrade (logo '', headline null → local walk, exhausted
    // false), never fail.
    const legacy = parseReport(JSON.stringify({entries: [{
        id: 'kimi', name: 'kimi', error: null,
        sections: [{type: 'metric', label: 'Rolling window (5h)', percent: 29, value: '29%',
            detail: '', severity: 'low', reset_at: '2026-09-02T14:00:00Z', window: 'session'}]
    }]}));
    assert.equal(legacy.ok, true, 'older binary report parses ok');
    assert.equal(legacy.entries[0].logo, '');
    assert.equal(legacy.entries[0].headline, null);
    assert.equal(legacy.entries[0].exhausted, false);
    assert.equal(headline(legacy.entries[0]).text, '29%',
        'null headline falls back to the local sections walk');

    // Caps: 70 entries truncate at 64, 100 sections at 96.
    const manyEntries = parseReport(JSON.stringify({entries: Array.from(
        {length: 70}, (_, i) => ({id: 'v' + i, error: null, sections: []}))}));
    assert.equal(manyEntries.ok, true);
    assert.equal(manyEntries.entries.length, 64, 'entries cap at 64');
    assert.equal(manyEntries.entries[63].id, 'v63');
    const manySections = parseReport(JSON.stringify({entries: [{id: 'v', error: null,
        sections: Array.from({length: 100}, () => ({type: 'spacer'}))}]}));
    assert.equal(manySections.entries[0].sections.length, 96, 'sections cap at 96');

    // Long strings are bounded: a 300-char display name lands at 240 code
    // units ending in the ellipsis.
    const longName = parseReport(JSON.stringify({entries: [{id: 'v', error: null,
        display_name: 'x'.repeat(300), sections: []}]}));
    const bounded = longName.entries[0].display_name;
    assert.equal(bounded.length, 240, 'display_name caps at 240 units');
    assert.ok(bounded.endsWith('…'));
    assert.equal(bounded.slice(0, 239), 'x'.repeat(239));
});

// ── remainingClass ────────────────────────────────────────────────────────
runGroup('remainingClass: bands flip strictly above 50/80/95 used', () => {
    // remaining = 100 - percent, strict < on remaining: the class flips at
    // the FIRST percent above the boundary, never on it.
    assert.equal(remainingClass(49), 'low', 'remainingClass(49): remaining 51');
    assert.equal(remainingClass(50), 'low', 'remainingClass(50): remaining exactly 50 stays low');
    assert.equal(remainingClass(51), 'mid', 'remainingClass(51): remaining 49 flips mid');
    assert.equal(remainingClass(79), 'mid', 'remainingClass(79): remaining 21');
    assert.equal(remainingClass(80), 'mid', 'remainingClass(80): remaining exactly 20 stays mid');
    assert.equal(remainingClass(81), 'high', 'remainingClass(81): remaining 19 flips high');
    assert.equal(remainingClass(94), 'high', 'remainingClass(94): remaining 6');
    assert.equal(remainingClass(95), 'high', 'remainingClass(95): remaining exactly 5 stays high');
    assert.equal(remainingClass(96), 'critical', 'remainingClass(96): remaining 4 flips critical');
    assert.equal(remainingClass(null), '', 'no percent → no class');
});

// ── compactReset ──────────────────────────────────────────────────────────
runGroup('compactReset: one-unit countdowns', () => {
    assert.equal(compactReset('2026-09-02T12:00:30Z', NOW), '1m', '30s rounds up to 1m');
    assert.equal(compactReset('2026-09-02T12:42:10Z', NOW), '43m', '42m10s ceils to 43m');
    assert.equal(compactReset('2026-09-02T12:59:59Z', NOW), '59m', '59m59s caps at 59m');
    assert.equal(compactReset('2026-09-02T17:18:00Z', NOW), '5.3h', '5h18m → 5.3h');
    assert.equal(compactReset('2026-09-02T17:00:00Z', NOW), '5h', 'exactly 5h drops the zero tenth');
    assert.equal(compactReset('2026-09-04T19:12:00Z', NOW), '2.3d', '55.2h → 2.3d');
    assert.equal(compactReset('2026-09-02T11:00:00Z', NOW), 'due', 'past reset reads due');
    assert.equal(compactReset('', NOW), '', 'no reset → empty string');
    assert.equal(compactReset(null, NOW), '', 'null reset → empty string');
    assert.equal(compactReset('not-a-date', NOW), '', 'unparseable reset → empty string');
});

// ── tileParts ─────────────────────────────────────────────────────────────
runGroup('tileParts: tags, windows, placeholder, balance fallback', () => {
    const mkEntry = (id, sections) => parseReport(JSON.stringify(
        {entries: [{id: id, logo: 'zai', error: null, sections: sections}]})).entries[0];
    const window = (percent, resetAt, win, secs) => ({type: 'metric', label: 'Window',
        percent: percent, value: percent + '%', detail: '', severity: 'low',
        reset_at: resetAt, window: win, window_seconds: secs ?? null});

    // Multi-account: the suffix distinguishes siblings; the tag carries it
    // both as text and as the label beside the logo.
    const multi = tileParts(mkEntry('zai@1', [window(42, '2026-09-02T14:05:00Z', 'session')]),
        true, false, NOW);
    assert.deepEqual(multi, [
        {text: '1', cls: '', logo: 'zai', logoLabel: '1'},
        {text: '42%', cls: 'low'},
        {text: '↺2h', cls: '', glue: true}
    ], 'multi-account tile keeps its suffix');

    // Solo: the provider's only entry drops the suffix; the provider name
    // becomes the tag text and the logo stands alone.
    const solo = tileParts(mkEntry('zai@1', [window(42, '2026-09-02T14:05:00Z', 'session')]),
        true, false, NOW, true);
    assert.deepEqual(solo, [
        {text: 'zai', cls: '', logo: 'zai', logoLabel: ''},
        {text: '42%', cls: 'low'},
        {text: '↺2h', cls: '', glue: true}
    ], 'solo tile degrades to the provider name');

    // A critical window carries cls 'critical' — bold is consumed at the
    // render site, never baked into the text.
    const critical = tileParts(mkEntry('kimi', [window(96, '2026-09-02T14:05:00Z', 'session')]),
        true, false, NOW);
    assert.equal(critical[1].cls, 'critical', 'critical window class');
    assert.equal(critical[1].text, '96%', 'critical text carries no bold markup');
    assert.equal(critical[2].text, '↺2h', 'the countdown is its own glued part');

    // A window not started yet keeps its slot as the dash placeholder.
    const unstarted = tileParts(mkEntry('kimi', [window(0, '', 'session')]), true, false, NOW);
    assert.equal(unstarted[1].text, '0%', 'unstarted window placeholder percent');
    assert.equal(unstarted[2].text, '↺-', 'unstarted window placeholder countdown');

    // Balance-style vendor (no windows): the single headline figure.
    const balance = tileParts(parseReport(JSON.stringify({entries: [{id: 'deepseek',
        error: null, sections: [{type: 'text', label: 'Balance', value: '$8.42'}]}]})).entries[0],
        true, false, NOW);
    assert.deepEqual(balance, [
        {text: 'deepseek', cls: '', logo: '', logoLabel: ''},
        {text: '$8.42', cls: ''}
    ], 'balance-style tile falls back to the headline figure');

    // A precomputed balance headline carries percent:null — showRemaining
    // must not flip null into "100%" (Number(null) is 0, so finitePercent
    // used to clamp it). The money figure survives the flip request.
    const withHeadline = tileParts(parseReport(JSON.stringify({entries: [{id: 'deepseek',
        error: null,
        headline: {text: '$8.42', percent: null, severity: 'low', label: 'Balance', reset_at: ''},
        sections: [{type: 'text', label: 'Balance', value: '$8.42'}]}]})).entries[0],
        true, true, NOW);
    assert.deepEqual(withHeadline, [
        {text: 'deepseek', cls: '', logo: '', logoLabel: ''},
        {text: '$8.42', cls: ''}
    ], 'null-percent headline keeps the balance figure under showRemaining');
});

// U9 (gnome): MiniMax exposes two text-pool windows AND optional video rows;
// tileParts only surfaces the two windows (session, weekly), never the video
// rows because they ship with window=null.
runGroup('tileParts: minimax video rows never tile, text windows do', () => {
    const mktwoWindow = (sess, weekly) => parseReport(JSON.stringify(
        {entries: [{id: 'minimax', logo: 'minimax', error: null, sections: [
            {type: 'metric', label: 'Session (5h)', percent: sess, value: sess + '%',
             detail: '', severity: 'low', reset_at: '2026-09-02T14:05:00Z', window: 'session',
             window_seconds: 5 * 3600},
            {type: 'metric', label: 'Weekly', percent: weekly, value: weekly + '%',
             detail: '', severity: 'low', reset_at: '2026-09-04T14:00:00Z', window: 'weekly',
             window_seconds: 7 * 24 * 3600}
        ]}]})).entries[0];
    const two = tileParts(mktwoWindow(25, 40), true, false, NOW);
    assert.deepEqual(two, [
        {text: 'minimax', cls: '', logo: 'minimax', logoLabel: ''},
        {text: '25%', cls: 'low'},
        {text: '↺2h', cls: 'mid', glue: true},
        {text: '40%', cls: 'low'},
        {text: '↺2d', cls: 'mid', glue: true}
    ], 'minimax text-pool windows tile in zai order; countdowns band on their own clocks');

    // Video rows follow the text rows but never become tile parts because
    // they carry window=null (Rust's metric_window rejects "video" labels).
    const withVideo = parseReport(JSON.stringify(
        {entries: [{id: 'minimax', logo: 'minimax', error: null, sections: [
            {type: 'metric', label: 'Session (5h)', percent: 25, value: '25%',
             detail: '', severity: 'low', reset_at: '2026-09-02T14:05:00Z', window: 'session'},
            {type: 'metric', label: 'Weekly', percent: 40, value: '40%',
             detail: '', severity: 'low', reset_at: '2026-09-04T14:00:00Z', window: 'weekly'},
            {type: 'metric', label: 'Video (5h)', percent: 30, value: '30%',
             detail: '', severity: 'low', reset_at: '2026-09-02T22:00:00Z', window: null},
            {type: 'metric', label: 'Video weekly', percent: 35, value: '35%',
             detail: '', severity: 'low', reset_at: '2026-09-07T14:00:00Z', window: null}
        ]}]})).entries[0];
    const parts = tileParts(withVideo, true, false, NOW);
    assert.deepEqual(parts, [
        {text: 'minimax', cls: '', logo: 'minimax', logoLabel: ''},
        {text: '25%', cls: 'low'},
        {text: '↺2h', cls: '', glue: true},
        {text: '40%', cls: 'low'},
        {text: '↺2d', cls: '', glue: true}
    ], 'video rows excluded even when present in the report');
});

// ── countdownClass ────────────────────────────────────────────────────────
runGroup('countdownClass: bands on the window clock, not the usage percent', () => {
    // 1h left of a 5h window = 20% of the clock → mid; 1.1d of 7d = 15.7% →
    // high; the countdown escalates independently of the percent beside it.
    assert.equal(countdownClass('2026-09-02T13:00:00Z', 5 * 3600, NOW), 'mid',
        '1h/5h = 20% of the clock left');
    assert.equal(countdownClass('2026-09-03T14:24:00Z', 7 * 24 * 3600, NOW), 'high',
        '1.1d/7d = 15.7% of the clock left');
    assert.equal(countdownClass('2026-09-02T16:30:00Z', 5 * 3600, NOW), 'low',
        '4.5h/5h = 90% of the clock left');
    assert.equal(countdownClass('2026-09-02T12:10:00Z', 5 * 3600, NOW), 'critical',
        '10m/5h = 3.3% of the clock left');
    assert.equal(countdownClass('2026-09-02T11:00:00Z', 5 * 3600, NOW), 'critical',
        'a past reset is out of clock');
    // Degradation: no window length (older binary, balance row) or an
    // unparsable reset leaves the countdown class-less (neutral).
    assert.equal(countdownClass('2026-09-02T13:00:00Z', null, NOW), '');
    assert.equal(countdownClass('', 5 * 3600, NOW), '');
    assert.equal(countdownClass('not-a-date', 5 * 3600, NOW), '');
});


// ── buildSettingsPatch ────────────────────────────────────────────────────
runGroup('buildSettingsPatch: valid keys and account changes', () => {
    const patch = buildSettingsPatch('openai', [
        {id: 'kimi', action: 'set', value: 'secret-value'},
        {id: 'zai', action: 'clear'}
    ], []);
    assert.equal(patch.ok, true);
    assert.deepEqual(JSON.parse(patch.payload), {
        schema_version: 1,
        primary: 'openai',
        keys: {
            kimi: {action: 'set', value: 'secret-value'},
            zai: {action: 'clear'}
        }
    });

    // Account add/update/remove, grouped per vendor in order.
    const accountPatch = buildSettingsPatch('', [], [
        {action: 'update', vendor: 'zai', label: 'team',
            fields: {organization_id: 'org-2'}, apiKey: {action: 'set', value: 'k'}},
        {action: 'add', vendor: 'zai', apiKey: {action: 'set', value: 'k2'}},
        {action: 'remove', vendor: 'zai', label: 'old'}
    ]);
    assert.equal(accountPatch.ok, true);
    assert.deepEqual(JSON.parse(accountPatch.payload).accounts.zai, [
        {action: 'update', label: 'team', fields: {organization_id: 'org-2'},
            api_key: {action: 'set', value: 'k'}},
        {action: 'add', api_key: {action: 'set', value: 'k2'}},
        {action: 'remove', label: 'old'}
    ]);

    // Blank vs clear is positional: KEY-section field values keep their
    // spaces exactly (blank = explicit value); ACCOUNT field values are
    // trimmed (a blank collapses to the empty string, still present).
    const keyBlank = buildSettingsPatch('', [
        {id: 'zai', action: 'fields', fields: {organization_id: ' '}}
    ], []);
    assert.deepEqual(JSON.parse(keyBlank.payload).keys.zai,
        {action: 'fields', fields: {organization_id: ' '}},
        'key-section field values are not trimmed');
    const accountTrim = buildSettingsPatch('', [], [
        {action: 'update', vendor: 'zai', label: 'team', fields: {organization_id: '  org  '}}
    ]);
    assert.deepEqual(JSON.parse(accountTrim.payload).accounts.zai[0].fields,
        {organization_id: 'org'}, 'account field values are trimmed');
    const accountBlank = buildSettingsPatch('', [], [
        {action: 'update', vendor: 'zai', label: 'team', fields: {organization_id: ' '}}
    ]);
    assert.deepEqual(JSON.parse(accountBlank.payload).accounts.zai[0].fields,
        {organization_id: ''}, 'a blank account field survives as the empty string');
});

runGroup('buildSettingsPatch: rejections', () => {
    const rejected = (primary, changes, accountChanges, label) => {
        const result = buildSettingsPatch(primary, changes, accountChanges);
        assert.equal(result.ok, false, label);
        assert.equal(result.payload, '', label + ': rejected patches carry no payload');
    };
    rejected('', [{id: 'bad id!', action: 'clear'}], [], 'invalid provider id rejected');
    rejected('openai', [{id: '__proto__', action: 'clear'}], [], '__proto__ row id rejected');
    rejected('', [{id: 'kimi', action: 'fields', fields: {'bad id!': 'x'}}], [],
        'invalid field id rejected');
    rejected('', [{id: 'kimi', action: 'fields', fields: {__defineGetter__: 'x'}}], [],
        '__defineGetter__ field key rejected');
    rejected('', [{id: 'kimi', action: 'fields', fields: {constructor: 'x'}}], [],
        'constructor field key rejected');
    rejected('', [{id: 'zai', action: 'fields', fields: {organization_id: 'x'.repeat(201)}}], [],
        'over-200-char field value rejected');
    rejected('', [{id: 'zai', action: 'fields', fields: {}}], [],
        'empty field map on a fields action rejected');
    rejected('', [{id: 'kimi', action: 'set', value: '   '}], [],
        'whitespace-only API key rejected');
    rejected('not a slug!', [], [], 'invalid primary rejected');
    rejected('', [], [], 'no primary, no keys, no accounts rejected');
    rejected('', [], [{action: 'explode', vendor: 'zai'}], 'invalid account action rejected');
    rejected('', [], [{action: 'add', vendor: 'bad id!'}], 'invalid account vendor rejected');
    rejected('', [], [{action: 'update', vendor: 'zai', label: 'x',
        apiKey: {action: 'set', value: ''}}], 'empty account API key rejected');
    // 16384 is the inclusive cap: the boundary itself passes, one more fails.
    assert.equal(buildSettingsPatch('', [{id: 'kimi', action: 'set',
        value: 'k'.repeat(16384)}], []).ok, true, '16384-char key is the inclusive cap');
    rejected('', [{id: 'kimi', action: 'set', value: 'k'.repeat(16385)}], [],
        '16385-char key rejected');
});

// ── parseSettingsSnapshot ─────────────────────────────────────────────────
runGroup('parseSettingsSnapshot: schema, caps, primary fallback', () => {
    const snapshot = parseSettingsSnapshot(JSON.stringify({
        schema_version: 1,
        primary: 'openai',
        primary_choices: [{id: 'anthropic', label: 'Claude'}, {id: 'openai', label: 'Codex'}],
        keys: [{id: 'zai', label: 'Z.AI', environment: 'ZAI_API_KEY', note: '',
            configured: true, inline_configured: true, environment_configured: false,
            fields: [
                {id: 'account_type', label: 'Account type', kind: 'choice', value: 'team',
                    choices: ['personal', 'team', 'usage']},
                {id: 'organization_id', label: 'Organization ID', kind: 'text', value: 'org-1'}
            ]}],
        accounts: [{vendor: 'zai', vendor_display: 'Z.AI', label: '', display: 'Default',
            environment: 'ZAI_API_KEY', configured: true, inline_configured: true,
            environment_configured: false,
            fields: [{id: 'api_key', label: 'API key', kind: 'secret', value: ''}]}]
    }));
    assert.equal(snapshot.ok, true);
    assert.equal(snapshot.primary, 'openai');
    assert.deepEqual(snapshot.primary_choices[0], {id: 'anthropic', value: 'anthropic',
        label: 'Claude'});
    assert.deepEqual(snapshot.keys[0].fields[0].choices, ['personal', 'team', 'usage']);
    assert.equal(snapshot.keys[0].fields[1].value, 'org-1');
    assert.equal(snapshot.accounts[0].display, 'Default');
    assert.equal(snapshot.accounts[0].fields[0].kind, 'secret');

    // schema_version is a hard gate.
    assert.equal(parseSettingsSnapshot(JSON.stringify(
        {schema_version: 2, primary_choices: [], keys: []})).ok, false,
        'schema_version 2 → ok:false');
    assert.equal(parseSettingsSnapshot('{').ok, false, 'malformed settings JSON → ok:false');

    // Caps: 65 choices → 64, 33 keys → 32, 13 fields → 12, 17 accounts → 16.
    const oversized = parseSettingsSnapshot(JSON.stringify({
        schema_version: 1,
        primary_choices: Array.from({length: 65}, (_, i) => ({id: 'c' + i})),
        keys: Array.from({length: 33}, (_, i) => ({id: 'k' + i,
            fields: Array.from({length: 13}, (_, j) => ({id: 'f' + j, kind: 'text', value: 'v'}))})),
        accounts: Array.from({length: 17}, (_, i) => ({vendor: 'zai', label: 'a' + i}))
    }));
    assert.equal(oversized.ok, true);
    assert.equal(oversized.primary_choices.length, 64, 'primary_choices cap at 64');
    assert.equal(oversized.keys.length, 32, 'keys cap at 32');
    assert.equal(oversized.keys[0].fields.length, 12, 'fields per key cap at 12');
    assert.equal(oversized.accounts.length, 16, 'accounts cap at 16');

    // A primary not among the choices falls back to the first choice.
    const fallback = parseSettingsSnapshot(JSON.stringify({
        schema_version: 1, primary: 'gone',
        primary_choices: [{id: 'anthropic', label: 'Claude'}, {id: 'openai', label: 'Codex'}],
        keys: []
    }));
    assert.equal(fallback.ok, true);
    assert.equal(fallback.primary, 'anthropic', 'primary falls back to the first choice');
});

// ── Static tables ─────────────────────────────────────────────────────────
runGroup('LOGO_ASSETS: 1:1 with gnome-extension/logos', () => {
    const keys = Object.keys(LOGO_ASSETS);
    assert.equal(keys.length, 16, '16 logo assets ship');
    for (const slug of keys)
        assert.ok(fs.existsSync(new URL('./logos/' + LOGO_ASSETS[slug], import.meta.url)),
            'logo asset ' + slug + ' → ' + LOGO_ASSETS[slug] + ' is missing');
    const shipped = fs.readdirSync(new URL('./logos/', import.meta.url)).sort();
    assert.deepEqual(shipped, [...new Set(Object.values(LOGO_ASSETS))].sort(),
        'every shipped logo file is mapped (no strays)');
});

runGroup('VENDOR_SLUGS: canonical 18 in VendorId enum order', () => {
    // The static list must mirror src/vendor.rs's VendorId slug table
    // exactly, enum order included.
    assert.equal(VENDOR_SLUGS.length, 18);
    assert.deepEqual([...VENDOR_SLUGS], [
        'anthropic', 'anthropic_api', 'openai', 'openai_api', 'zai', 'openrouter',
        'deepseek', 'kimi', 'kilo', 'novita', 'moonshot', 'grok', 'supergrok',
        'antigravity', 'cursor', 'minimax', 'kiro', 'opencode-go'
    ]);
});

// ── Parse gates ───────────────────────────────────────────────────────────
// extension.js/prefs.js/subprocess-utils.js are plain GNOME-45+ ESM: temp
// .mjs copies let `node --check` parse ESM without resolving the gi://
// imports the shell provides.
runGroup('parse gates: extension.js, prefs.js, subprocess-utils.js parse as ESM', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'aiub-parse-gate-'));
    try {
        for (const name of ['extension.js', 'prefs.js', 'subprocess-utils.js']) {
            const copy = path.join(dir, name.replace(/\.js$/, '.mjs'));
            fs.copyFileSync(new URL('./' + name, import.meta.url), copy);
            const check = spawnSync(process.execPath, ['--check', copy], {encoding: 'utf8'});
            assert.equal(check.status, 0,
                name + ' must stay syntactically valid ESM' + (check.stderr ? ': ' + check.stderr : ''));
            assert.equal(check.stderr, '', name + ' --check stderr must be empty');
        }
    } finally {
        fs.rmSync(dir, {recursive: true, force: true});
    }
    assert.equal(fs.existsSync(dir), false, 'temp parse-gate dir cleaned up');
});

// ── subprocess-utils structural gates ─────────────────────────────────────
// node cannot import gi:// modules, so these gates assert the FILE TEXT:
// subprocess-utils.js must load in BOTH the gnome-shell process and the
// prefs process, which it only does while its imports stay Gio/GLib-only.
runGroup('subprocess-utils: imports limited to gi://Gio and gi://GLib', () => {
    const src = fs.readFileSync(new URL('./subprocess-utils.js', import.meta.url), 'utf8');
    assert.ok(!src.includes('require('), 'no CommonJS require( — ESM only');
    // Static import/export-from statements at line starts; the clause stays
    // single-line so an `export function …` declaration can never be read as
    // an export-from by swallowing the next line's first string literal.
    const specifiers = [...src.matchAll(/(?:^|\n)\s*(?:import|export)\s[^;'"\n]*?(?:from\s*)?['"]([^'"]+)['"]/g)]
        .map(m => m[1]);
    assert.ok(specifiers.length > 0, 'subprocess-utils.js must import Gio/GLib at all');
    for (const spec of specifiers)
        assert.ok(spec === 'gi://Gio' || spec === 'gi://GLib',
            'unexpected import "' + spec + '" — subprocess-utils may only import gi://Gio and gi://GLib so both the shell and prefs processes can load it');
    assert.ok(!specifiers.some(spec => spec.startsWith('.')),
        'no relative imports — they would couple this module to shell-side files');
});

runGroup('subprocess-utils: null-stdin guard and BROKEN_PIPE mapping present', () => {
    const src = fs.readFileSync(new URL('./subprocess-utils.js', import.meta.url), 'utf8');
    // Textual, not executed (node cannot load gi://): the guard must refuse
    // a missing payload WITHOUT spawning, resolving instead of rejecting.
    assert.ok(/stdinText\s*===\s*null\s*\|\|\s*stdinText\s*===\s*undefined/.test(src),
        'runWithStdin must guard stdinText === null || stdinText === undefined before spawning');
    assert.ok(src.includes('internal error: stdin payload missing'),
        'the null-stdin guard resolves with the "internal error: stdin payload missing" result');
    assert.ok(src.includes('Gio.IOErrorEnum.BROKEN_PIPE'),
        'runWithStdin must match Gio.IOErrorEnum.BROKEN_PIPE');
    assert.ok(src.includes('binary exited before reading the patch'),
        'BROKEN_PIPE maps to the readable "binary exited before reading the patch" error');
});

runGroup('subprocess-utils: portable exit-code read', () => {
    const src = fs.readFileSync(new URL('./subprocess-utils.js', import.meta.url), 'utf8');
    // GLib 2.88 (gjs 1.88, GNOME 49+) dropped get_exit_code from the
    // introspected surface; calling it there throws the TypeError that
    // killed the whole Accounts page. The exit code must go through a
    // helper that tries get_exit_status first and only falls back to
    // get_exit_code on older runtimes.
    assert.ok(/typeof\s+p\.get_exit_status\s*===\s*'function'/.test(src),
        'exit-code read must branch on typeof p.get_exit_status (GLib 2.88+)');
    assert.ok(/exitCode:\s*exitCodeOf\(p\)/.test(src),
        '_collect must read the exit code through exitCodeOf(p), never p.get_exit_code() directly');
    assert.ok(!/p\.get_exit_code\(\)/.test(src.replace(/function exitCodeOf\(p\) \{[\s\S]*?\n\}/, '')),
        'no direct p.get_exit_code() call outside the compatibility helper');
});

// resolveBinary is the READER of ./scripts/install-bin.sh's copies: the
// writer may land the binaries in any of its destination dirs, and the
// gnome-shell process PATH (a systemd user environment) often misses them
// until the next login — so a "Refresh now" after installing must still
// find the binary through explicit file checks, without a shell restart.
runGroup('subprocess-utils: resolveBinary knows every install-bin destination', () => {
    const src = fs.readFileSync(new URL('./subprocess-utils.js', import.meta.url), 'utf8');
    const resolver = src.slice(src.indexOf('export function resolveBinary'),
        src.indexOf('export function runJson'));
    for (const marker of ['.local/bin', '.cargo/bin', 'mise/shims'])
        assert.ok(resolver.includes(marker),
            'resolveBinary must file-test ' + marker +
            ' — install-bin.sh may have copied the binary there');
    assert.ok(resolver.includes('find_program_in_path'),
        'the session PATH stays the first auto-detection step');
});

// ── libadwaita 2.0 (GNOME 49+) widget-API gates ────────────────────────────
// Two removed APIs each aborted the whole Accounts page mid-render, leaving
// only the Primary provider group; node cannot construct Adw widgets, so pin
// the portable call shapes textually.
runGroup('prefs: libadwaita 2.0 widget API portability', () => {
    const src = fs.readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');
    assert.ok(!/row\.add\(/.test(src),
        'AdwExpanderRow children must go through add_row() — the legacy add() is gone in libadwaita 2.0 and its TypeError aborted the Accounts page after the primary group');
    assert.ok(/typeof keyEntry\.show_peek_icon === 'boolean'/.test(src),
        'show-peek-icon must be feature-tested after construction — passing it to the constructor throws "No property show_peek_icon" on libadwaita 2.0 (the eye is always shown there)');
});

runGroup('extension: omarchy-style bar plate wired', () => {
    const css = fs.readFileSync(new URL('./stylesheet.css', import.meta.url), 'utf8');
    const js = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(css.includes('.aiub-tile-plate'),
        'stylesheet must define the .aiub-tile-plate backdrop (the QML BarWidget plate ported)');
    assert.ok(/add_style_class_name\('aiub-tile-plate'\)/.test(js)
        && /remove_style_class_name\('aiub-tile-plate'\)/.test(js),
        '_updateBar must plate the tile row in tiled mode and unplate the fallback states');
});

// ── prefs structural gates (todo 5) ────────────────────────────────────────
// The Accounts page is data-driven over the settings bridge: these gates pin
// the wiring invariants at the source-text level (node cannot import gi://
// modules); the parse gate above keeps prefs.js syntactically valid ESM.
runGroup('prefs: accounts page wired to the settings bridge', () => {
    const src = fs.readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');
    assert.ok(/['"]\.\/subprocess-utils\.js['"]/.test(src),
        'prefs.js must import ./subprocess-utils.js (resolveBinary/runJson/runWithStdin)');
    assert.ok(!/config\.toml/.test(src) && !/file_get_contents|get_contents/.test(src),
        'prefs.js must not mention config.toml or read configuration files — the Rust settings bridge owns the file (THIN invariant)');
    assert.ok(!src.includes('VENDOR_AUTH'),
        'the hardcoded VENDOR_AUTH table is gone — accounts come from `settings show`');
});

// ── prefs structural gates (todo 9: display page rebuild) ─────────────────
// The display page must only touch keys the schema declares: the legacy
// Waybar-format keys are gone from the schema, and a row, bind, or
// connection left on any undeclared key would crash the prefs dialog the
// moment the compiled schema stops providing it. The gschema.xml is the
// source of truth, so the gate derives the key set from it rather than
// hardcoding names — it fails on ANY undeclared key prefs.js touches
// (dynamic `changed::${key}` template strings are covered by the literal
// call sites that feed them).
runGroup('prefs: every GSettings key touched exists in the schema', () => {
    const src = fs.readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');
    const schema = fs.readFileSync(
        new URL('./schemas/org.gnome.shell.extensions.ai-usagebar.gschema.xml', import.meta.url), 'utf8');
    const declared = new Set([...schema.matchAll(/<key name=['"]([^'"]+)['"]/g)].map(m => m[1]));
    assert.equal(declared.size, 15,
        'the schema declares its 15 post-cleanup keys (found ' + declared.size + ')');
    const touched = new Set();
    for (const m of src.matchAll(/(?:bind|get_[a-z]+|set_[a-z]+)\(\s*'([^']+)'/g))
        touched.add(m[1]);
    for (const m of src.matchAll(/changed::([a-z0-9-]+)/g))
        touched.add(m[1]);
    const missing = [...touched].filter(key => !declared.has(key));
    assert.deepEqual(missing, [],
        'prefs.js references GSettings keys the schema does not declare (found: ' +
        (missing.length > 0 ? missing.join(', ') : 'none') + ')');
});

// ── extension.js structural gates (todo 7) ────────────────────────────────
// The popup now projects the usage report: node cannot import gi:// modules,
// so these gates pin the wiring invariants at the source-text level (the
// parse gate above keeps extension.js syntactically valid ESM).
runGroup('extension: popup imports subprocess-utils and fetches the usage report', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(/['"]\.\/subprocess-utils\.js['"]/.test(src),
        'extension.js must import ./subprocess-utils.js (resolveBinary/runJson)');
    assert.ok(/'usage'\s*,\s*'--json'/.test(src),
        "the fetch argv is [resolveBinary(...), 'usage', '--json']");
});

runGroup('extension: no settings-apply spawn from the shell process', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    // Secrets live in the prefs process only: an apply spawn from the shell
    // would have to carry its patch in argv or the environment.
    assert.ok(!/['"]settings['"]\s*,\s*['"]apply['"]/.test(src),
        'extension.js must never spawn `settings apply`');
});

runGroup('extension: the marker pipeline is gone (no FORMAT/FIELD references)', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    // Word-boundary + case-insensitive: catches the old FORMAT/FIELD
    // constants AND a re-added '--format' argv string, while composite
    // identifiers like formatReset (legitimate report-model helpers) pass.
    assert.ok(!/\b(?:format|field)\b/i.test(src),
        'no format/field marker-pipeline references may remain in extension.js');
});

runGroup('extension: every GLib source id is torn down in destroy()/disable()', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    // Teardown region: from the first destroy()/disable() method to the end
    // of the file, which covers every teardown body the file defines.
    const teardownAt = src.search(/\n {4}(?:destroy|disable)\(\) \{/);
    assert.ok(teardownAt >= 0, 'extension.js must define a destroy()/disable() teardown');
    const teardown = src.slice(teardownAt);

    const calls = [...src.matchAll(/GLib\.(?:timeout|idle)_add\w*/g)];
    assert.ok(calls.length > 0, 'extension.js registers at least one periodic GLib source');
    for (const call of calls) {
        const prefix = src.slice(Math.max(0, call.index - 96), call.index);
        assert.ok(/(?:this\._\w+|(?:const|let|var)\s+\w+)\s*=\s*$/.test(prefix),
            'every GLib timeout/idle source must store its id (' + call[0] + ' is unnamed)');
    }

    const assigned = [...src.matchAll(
        /(this\._\w+|(?:const|let|var)\s+\w+)\s*=\s*GLib\.(?:timeout|idle)_add\w*/g)]
        .map(m => m[1]);
    for (const name of assigned) {
        // Fire-once locals are scoped to their callback; instance sources
        // outlive the call and must be referenced in teardown.
        if (!name.startsWith('this._'))
            continue;
        assert.ok(teardown.includes(name),
            name + ' is assigned a GLib source but is never referenced inside destroy()/disable()');
    }
    assert.ok(teardown.includes('GLib.source_remove(this._tickerId)'),
        'the 30s countdown ticker (this._tickerId) must be removed in teardown');
});

// ── extension.js structural gates (todo 8: the tiled bar) ─────────────────
// The bar tiles every working account, composed through report-model
// helpers only (node cannot import gi://, so these pin the wiring at the
// source-text level; the parse gate above keeps extension.js valid ESM).
runGroup('extension: tiled bar consumes display keys, selection, and tile helpers', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    for (const key of ['show-logos', 'bar-tiled', 'show-remaining'])
        assert.ok(src.includes(`get_boolean('${key}')`),
            `the bar must read the ${key} key live from settings`);
    assert.ok(src.includes("get_string('last-selected-entry')"),
        'the remembered selection (last-selected-entry) must be read');
    assert.ok(src.includes("set_string('last-selected-entry',"),
        'user-driven selection changes must persist last-selected-entry');
    for (const name of ['barTileEntries', 'tileParts', 'tileTag', 'providerEntryCounts',
        'selectedIndex', 'preferredEntryId', 'logoAssetName', 'baseProvider'])
        assert.ok(src.includes(name),
            `the bar composes through report-model's ${name} (THIN invariant)`);
    assert.ok(!src.includes('_buttonLabel'),
        'the interim single-label button is gone — tiles own the bar now');
});

runGroup('extension: wheel and middle-click cycle the bar; no right-click handler', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(/connect\(\s*'scroll-event'/.test(src),
        'the button actor must connect scroll-event for wheel cycling');
    assert.ok(/connect\(\s*'button-press-event'/.test(src),
        'the button actor must connect button-press-event for middle click');
    assert.ok(/get_button\(\)\s*===\s*2/.test(src),
        'button 2 (middle click) selects the next entry');
    assert.ok(!/get_button\(\)\s*(?:===|==|!==|!=)\s*3/.test(src)
        && !/\bbutton\s*(?:===|==)\s*3\b/.test(src),
        'no right-click (button 3) handler may exist');
});

// GJS ≥1.78 (GNOME Shell 49+) dropped the Gio.FileIcon.new_for_file static —
// it loads fine syntactically but throws at enable time, which node cannot
// catch. Pin the constructor form; ban the static everywhere in the extension.
runGroup('extension: FileIcon built via constructor (no new_for_file static)', () => {
    for (const name of ['extension.js', 'prefs.js']) {
        const src = fs.readFileSync(new URL('./' + name, import.meta.url), 'utf8');
        assert.ok(!src.includes('FileIcon.new_for_file'),
            name + ' must not use Gio.FileIcon.new_for_file — the static does not ' +
            'exist in newer GJS and throws at runtime');
    }
    const ext = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(ext.includes('new Gio.FileIcon({file:'),
        'extension.js must build file icons with new Gio.FileIcon({file: …})');
});

// ── accountTabs (the dropdown's per-key tabs) ──────────────────────────────
// The dropdown tiles ONE TAB PER KEY (account), not per vendor: tabs are
// sorted by vendor first (a vendor's keys stay adjacent), then by key —
// the default account leads, numbered siblings compare numerically. Only
// accounts that actually READ get a tab (the omarchy panel's rule);
// unconfigured and broken entries never do.
runGroup('accountTabs: one tab per key, sorted by vendor then key', () => {
    const parsed = parseReport(JSON.stringify({primary: 'zai', entries: [
        {id: 'zai@2', display_name: 'Z.AI · 2', error: null,
            sections: [{type: 'metric', label: 'Weekly', percent: 28, value: '28%',
                window: 'weekly'}]},
        {id: 'openrouter', display_name: 'OpenRouter', error: 'no API key',
            unconfigured: true},
        {id: 'kimi', display_name: 'Kimi', error: null,
            sections: [{type: 'metric', label: 'Session (5h)', percent: 62, value: '62%',
                window: 'session'}]},
        {id: 'zai@1', display_name: 'Z.AI · 1', error: null,
            sections: [{type: 'metric', label: 'Session (5h)', percent: 1, value: '1%',
                window: 'session'}]},
        {id: 'deepseek', display_name: 'DeepSeek', error: 'network down',
            unconfigured: false},
    ]}));
    const tabs = accountTabs(parsed.entries);
    assert.deepEqual(tabs.map(entry => entry.id), ['kimi', 'zai@1', 'zai@2'],
        'ready keys only, vendor-grouped, keys ascending; unconfigured openrouter ' +
        'and broken deepseek get no tab');
    assert.deepEqual(tabs.map(entry => providerName(entry)), ['Kimi', 'Z.AI · 1', 'Z.AI · 2'],
        'a key tab is labeled by the account\'s own display_name');
});

runGroup('accountTabs: numeric keys, default-first, degenerate inputs', () => {
    const parse = entries => parseReport(JSON.stringify({entries})).entries;
    assert.deepEqual(
        accountTabs(parse([
            {id: 'zai@10', error: null, sections: []},
            {id: 'zai@2', error: null, sections: []},
            {id: 'zai@1', error: null, sections: []},
        ])).map(entry => entry.id),
        ['zai@1', 'zai@2', 'zai@10'],
        'numbered keys compare numerically (1 < 2 < 10, not lexicographic)');
    assert.deepEqual(
        accountTabs(parse([
            {id: 'kimi@beta', error: null, sections: []},
            {id: 'kimi', error: null, sections: []},
            {id: 'kimi@alpha', error: null, sections: []},
        ])).map(entry => entry.id),
        ['kimi', 'kimi@alpha', 'kimi@beta'],
        'the vendor default (no @) leads; named keys compare as text');
    assert.deepEqual(
        accountTabs(parse([
            {id: 'zai@1', error: null, exhausted: true, sections: []},
        ])).map(entry => entry.id),
        ['zai@1'],
        'an exhausted account keeps its tab — tabs and popups never drop it');
    assert.deepEqual(accountTabs([]), []);
    assert.deepEqual(accountTabs(null), []);
});

// ── extension.js structural gates (the per-key tab dropdown) ───────────────
runGroup('extension: per-key tabs and the settings entry wired in the dropdown', () => {
    const js = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(/import\s*\{[^}]*accountTabs[^}]*\}\s*from\s*['"]\.\/report-model\.js['"]/.test(js),
        'extension.js must import accountTabs from the adapter (THIN invariant)');
    assert.ok(/accountTabs\(entries\)/.test(js) && /_addTabRow\(/.test(js),
        '_buildMenu tiles one tab per key through accountTabs and renders the row');
    assert.ok(/preferences-system-symbolic/.test(js) && /_openPrefs\(\)/.test(js),
        'the tab row carries a gear button wired to Preferences (its Accounts page sets keys)');
    // Unconfigured vendors must not render in the dropdown body: the menu
    // loop only routes status "error" to the remedy row anymore.
    const menuBody = js.slice(js.indexOf('_buildMenu()'), js.indexOf('_addFooter('));
    assert.ok(/entry\.status\s*===\s*'error'/.test(menuBody)
        && !/entry\.status\s*!==\s*'ready'/.test(menuBody),
        'only configured-but-broken (error) entries keep a calm row; unconfigured ones are hidden');
    const css = fs.readFileSync(new URL('./stylesheet.css', import.meta.url), 'utf8');
    for (const cls of ['.aiub-tab', '.aiub-tab-active', '.aiub-tab-gear'])
        assert.ok(css.includes(cls), 'stylesheet must define ' + cls);
});

// St.Align left the St typelib in GNOME 49/50 (St-17/18): the reference is
// undefined at runtime and kills the first metric row's constructor, which
// aborts _buildMenu after the tab row — tabs render, usage rows never do.
runGroup('extension: no St.Align references (dead in GNOME 49/50)', () => {
    const js = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(!js.includes('St.Align'),
        'extension.js must not reference St.Align — the enum is gone from St-17/18 ' +
        'and its dead reference aborts the dropdown content mid-build');
    assert.ok(/Clutter\.ActorAlign\.START/.test(js),
        'the meter track aligns through Clutter.ActorAlign.START, the living enum');
});

// A St.Bin packs its single child through ClutterBinLayout, whose alignment
// semantics shifted across shell generations (and whose centered default
// produced a floating stub). The meter therefore avoids child alignment
// entirely: a horizontal St.BoxLayout track packs the fill from x=0 —
// progress anchors left by construction.
runGroup('extension: meter fill packed from the left edge', () => {
    const js = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(/const track = new St\.BoxLayout\(\{[\s\S]{0,220}?aiub-meter-track/.test(js),
        'the meter track must be a horizontal St.BoxLayout (fixed width) — packing, ' +
        'not child alignment, anchors the fill');
    assert.ok(/const fill = new St\.Bin\(\{[\s\S]{0,260}?aiub-meter-fill/.test(js)
        && /track\.add_child\(fill\)/.test(js),
        'the fill bin is added as the track BoxLayout child — the first packed child ' +
        'occupies the left N pixels regardless of alignment defaults');
});

// The dropdown's bottom items are Refresh now and Preferences only; the
// terminal TUI launcher lives outside the shell menu by design.
runGroup('extension: no Open TUI item in the dropdown', () => {
    const js = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(!js.includes('Open TUI') && !js.includes('_openTui'),
        'the dropdown must not carry an Open TUI item or a terminal launcher');
});

// The prefs window's first page needs a title (Common) or the page switcher
// shows a blank entry, and every color row spells out its remaining band in
// the same full-text form as Low (at least 50% remaining).
runGroup('prefs: Common page title and full-text color band labels', () => {
    const src = fs.readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');
    assert.ok(/new Adw\.PreferencesPage\(\{title: _\('Common'\)\}\)/.test(src),
        "the first preferences page must be titled 'Common'");
    for (const label of ['at least 50% remaining', 'below 50% remaining',
        'below 20% remaining', 'below 5% remaining', 'meter track'])
        assert.ok(src.includes(label), 'a color row must carry the band text: ' + label);
    assert.ok(!/<\d+% remaining/.test(src),
        'no shorthand (<N% remaining) color labels — every row uses the full-text form');
});

// The Add-account draft card opens expanded: it exists to be filled in, so a
// collapsed ExpanderRow costs the user an extra click before typing.
runGroup('prefs: new-account draft card opens expanded', () => {
    const src = fs.readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');
    assert.ok(/new Adw\.ExpanderRow\(\{[\s\S]{0,160}?expanded: true/.test(src),
        'the draft ExpanderRow must construct with expanded: true — its fields ' +
        'open with the card');
});

// The install scripts deploy a REAL COPY of the extension: a symlinked
// checkout dangles the moment the project directory moves and the installed
// extension stops loading. Both entry points (repo-root and in-dir) must
// copy the files into the extensions directory.
runGroup('install scripts copy the extension, never symlink', () => {
    for (const rel of ['../scripts/install-gnome.sh', './install.sh']) {
        const src = fs.readFileSync(new URL(rel, import.meta.url), 'utf8');
        assert.ok(!/\bln\s+-s\b/.test(src),
            rel + ' must not symlink the extension — moving the checkout dangles ' +
            'the link and breaks the installed plugin');
        assert.ok(/\bcp\s+-a\b/.test(src),
            rel + ' must copy the extension files into the extensions directory');
    }
});

// The Rust executables get their own installer, and every frontend
// installer references it: the extension/plugin spawns
// `ai-usagebar-omarchy` from the session PATH, so a deploy that skips the
// binaries ships a broken surface. The omarchy entry point is named for
// its platform (install-omarchy.sh, never the old bare install.sh).
runGroup('install scripts: install-bin.sh owns the binary deploy', () => {
    const bin = fs.readFileSync(new URL('../scripts/install-bin.sh', import.meta.url), 'utf8');
    for (const name of ['ai-usagebar-omarchy', 'ai-usagebar-omarchy-tui'])
        assert.ok(bin.includes(name), 'install-bin.sh must deploy ' + name);
    assert.ok(/install -Dm755/.test(bin) && !/\bln\s+-s\b/.test(bin),
        'install-bin.sh copies real files (install -Dm755), never a link into target/');
    assert.ok(/"\$REPO\/scripts\/build\.sh"/.test(bin),
        'install-bin.sh must compile first by invoking "$REPO/scripts/build.sh" — a ' +
        'fresh checkout deploys without a separate manual build step');
    for (const rel of ['../scripts/install-omarchy.sh', '../scripts/install-gnome.sh']) {
        const src = fs.readFileSync(new URL(rel, import.meta.url), 'utf8');
        assert.ok(/install-bin\.sh/.test(src),
            rel + ' must reference install-bin.sh so its deploy installs the binaries');
    }
    assert.ok(!fs.existsSync(new URL('../scripts/install.sh', import.meta.url)),
        'the omarchy installer is scripts/install-omarchy.sh — the old bare ' +
        'scripts/install.sh must not come back');
});

// ── alarm isolation ───────────────────────────────────────────────────────
runGroup('alarm isolation: one vendor\'s urgency never recolors another tile', () => {
    // The live incident: kimi's 5h window is maxed (critical headline,
    // exhausted), zai/minimax/deepseek are calm. The old bar-wide _alarming
    // reddened EVERY tile's neutral text — zai's "1"/"2" suffix, minimax's
    // tag, deepseek's whole money figure — through .aiub-alarm.
    const parsed = parseReport(JSON.stringify({entries: [
        {
            id: 'zai@1', name: 'zai · 1', display_name: 'Z.AI · 1', logo: 'zai',
            status: 'ready', stale: false,
            headline: {text: '62%', percent: 62, severity: 'mid', label: 'Session (5h)'},
            sections: [
                {type: 'metric', label: 'Session (5h)', percent: 62, value: '62%',
                    severity: 'mid', window: 'session'},
                {type: 'metric', label: 'Weekly', percent: 55, value: '55%',
                    severity: 'mid', window: 'weekly'}
            ]
        },
        {
            id: 'minimax@1', name: 'minimax · 1', display_name: 'MiniMax', logo: 'minimax',
            status: 'ready', stale: false,
            headline: {text: '1%', percent: 1, severity: 'low', label: 'Session (5h)'},
            sections: [
                {type: 'metric', label: 'Session (5h)', percent: 1, value: '1%',
                    severity: 'low', window: 'session'},
                {type: 'metric', label: 'Weekly', percent: 2, value: '2%',
                    severity: 'low', window: 'weekly'}
            ]
        },
        {
            id: 'deepseek@1', name: 'deepseek · 1', display_name: 'DeepSeek', logo: 'deepseek',
            status: 'ready', stale: false,
            headline: {text: '¥58.40', percent: null, severity: 'low', label: 'Balance'},
            sections: [{type: 'text', label: 'Balance', value: '¥58.40'}]
        },
        {
            id: 'kimi', name: 'kimi', display_name: 'Kimi', logo: 'kimi',
            status: 'ready', stale: false, exhausted: true,
            headline: {text: '100%', percent: 100, severity: 'critical',
                label: 'Rolling window (5h)'},
            sections: [
                {type: 'metric', label: 'Rolling window (5h)', percent: 100, value: '100%',
                    severity: 'critical', window: 'session'},
                {type: 'metric', label: 'Weekly quota', percent: 20, value: '20%',
                    severity: 'low', window: 'weekly'}
            ]
        }
    ]}));
    const [zai, minimax, deepseek, kimi] = parsed.entries;
    assert.equal(tileAlarmClass(kimi), 'aiub-alarm',
        'the maxed account still alarms its own tile');
    assert.equal(tileAlarmClass(zai), '', 'zai\'s tag keeps its neutral color');
    assert.equal(tileAlarmClass(minimax), '', 'minimax\'s tag keeps its neutral color');
    assert.equal(tileAlarmClass(deepseek), '',
        'the balance figure keeps its neutral color');

    // Stale data alarms its OWN tile too — isAlarming's other real half.
    const stale = parseReport(JSON.stringify({entries: [{
        id: 'zai', name: 'zai', display_name: 'Z.AI', status: 'ready', stale: true,
        headline: {text: '10%', percent: 10, severity: 'low', label: 'Session (5h)'},
        sections: [{type: 'metric', label: 'Session (5h)', percent: 10, value: '10%',
            severity: 'low', window: 'session'}]
    }]})).entries[0];
    assert.equal(tileAlarmClass(stale), 'aiub-alarm', 'a stale account alarms its own tile');

    // The robot (single-account) label: bar-wide failures or the SHOWN
    // account's own urgency — never a sibling vendor's critical headline.
    assert.equal(robotAlarmClass('', false, zai), '', 'calm shown account, calm robot');
    assert.equal(robotAlarmClass('', false, kimi), 'aiub-alarm',
        'the shown account\'s urgency reddens its own robot');
    assert.equal(robotAlarmClass('usage command failed', false, zai), 'aiub-alarm',
        'a refresh failure still alarms');
    assert.equal(robotAlarmClass('', true, null), 'aiub-alarm',
        'a filter miss alarms the robot');
    assert.equal(robotAlarmClass('', false, null), '', 'plain loading stays calm');
});

// Structural gate: the render layer must keep the per-tile wiring — a
// bar-wide alarm class applied to every tile label is exactly the regression
// that painted zai's suffix and deepseek's balance red while kimi was maxed.
runGroup('extension.js wiring: tiles alarm per account, never bar-wide', () => {
    const src = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
    assert.ok(src.includes('tileAlarmClass(entry)'),
        'each tile label carries its own account\'s alarm class');
    assert.ok(!src.includes('worstBand'),
        'no bar-wide worst-band alarm source may remain in the render layer');
    assert.ok(!/_alarming\b/.test(src),
        'the bar-wide _alarming walker must not come back');
});

for (const line of summary) console.log(line);
if (failures > 0) {
    console.error(failures + ' report-model test group(s) failed');
    process.exitCode = 1;
} else {
    console.log('report-model tests passed');
}
