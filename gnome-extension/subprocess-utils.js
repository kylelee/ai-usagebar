// Subprocess helpers shared by the gnome-shell process (extension.js) and
// the prefs process (prefs.js). Both processes can load this file because
// it depends on nothing shell-specific: only gi://Gio and gi://GLib, which
// exist in both processes. Never import extension.js, prefs.js, St, or any
// resource:/// module here; that would make the file prefs-unloadable.
//
// `settings apply` payloads travel over stdin ONLY — never argv, never the
// environment — so runWithStdin is the one sanctioned transport for them.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

// Single home for the binary lookup that extension.js and prefs.js each
// carried a copy of. `settings` is the extension's GSettings object
// (get_string('binary-path')).
export function resolveBinary(settings) {
    const configured = settings.get_string('binary-path');
    if (configured && GLib.file_test(configured, GLib.FileTest.IS_EXECUTABLE))
        return configured;
    // The READER of ./scripts/install-bin.sh's copies: the writer may land
    // the binaries in any of its destination dirs (first writable of
    // ~/.local/bin, ~/.cargo/bin, mise shims), and the gnome-shell process
    // PATH — a systemd user environment — often misses those until the next
    // login. File-test every destination so a "Refresh now" picks the
    // binary up right after install, with no shell restart.
    const installDirs = [
        `${GLib.get_home_dir()}/.local/bin`,
        `${GLib.get_home_dir()}/.cargo/bin`,
        `${GLib.get_user_data_dir()}/mise/shims`,
    ];
    // The canonical name first; the legacy short name still resolves on AUR
    // installs, which ship a compatibility symlink.
    for (const name of ['ai-usagebar-omarchy', 'ai-usagebar']) {
        const onPath = GLib.find_program_in_path(name);
        if (onPath)
            return onPath;
        for (const dir of installDirs) {
            const candidate = `${dir}/${name}`;
            if (GLib.file_test(candidate, GLib.FileTest.IS_EXECUTABLE))
                return candidate;
        }
    }
    return 'ai-usagebar-omarchy';
}

// g_subprocess_get_exit_code() left the introspected surface in GLib 2.88
// (gjs 1.88, GNOME 49+): the decoded value is get_exit_status() there. Read
// whichever the runtime exposes; a force-exited (signaled) process has no
// exit code on either API, so it maps to the shell's 128+signal convention.
function exitCodeOf(p) {
    if (typeof p.get_exit_status === 'function')
        return p.get_if_exited() ? p.get_exit_status() : 128 + p.get_term_sig();
    return p.get_exit_code();
}

// Shared never-reject spawn engine. `stdinText` is null when no stdin is
// wanted (no STDIN_PIPE is requested; communicate then reads nothing) or
// the exact string to write for runWithStdin. Every outcome — success,
// non-zero exit, spawn failure, cancellation, timeout — resolves into the
// result object; the promise NEVER rejects.
function _collect(argv, stdinText, timeoutSec, cancellable) {
    return new Promise(resolve => {
        let proc;
        try {
            proc = new Gio.Subprocess({
                argv,
                flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE |
                    (stdinText === null ? 0 : Gio.SubprocessFlags.STDIN_PIPE),
            });
            proc.init(cancellable);
        } catch (e) {
            resolve({ok: false, exitCode: -1, stdout: '', stderr: '',
                timedOut: false, error: `spawn failed: ${e}`});
            return;
        }

        let timedOut = false;
        let timeoutId = 0;
        let settled = false;
        const finish = result => {
            if (settled)
                return;
            settled = true;
            // Remove the still-armed source on every settling path; the
            // timeout callback clears `timeoutId` itself before returning
            // SOURCE_REMOVE, so a fired source is never removed twice.
            if (timeoutId) {
                GLib.source_remove(timeoutId);
                timeoutId = 0;
            }
            resolve(result);
        };

        // Force-exit-on-timeout: the process is killed, not merely waited
        // on, so a hung binary can never pin a slot forever.
        timeoutId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, timeoutSec, () => {
            timeoutId = 0;
            timedOut = true;
            try {
                proc.force_exit();
            } catch (e) {}
            if (cancellable && !cancellable.is_cancelled())
                cancellable.cancel();
            return GLib.SOURCE_REMOVE;
        });

        proc.communicate_utf8_async(stdinText, cancellable, (p, res) => {
            try {
                const [, out, err] = p.communicate_utf8_finish(res);
                const ok = !timedOut && p.get_successful();
                finish({
                    ok,
                    exitCode: exitCodeOf(p),
                    stdout: out || '',
                    stderr: err || '',
                    timedOut,
                    // A non-zero exit keeps error empty: the caller reads
                    // exitCode/stderr; `error` is for cases with no output.
                    error: timedOut ? `timeout after ${timeoutSec}s` : '',
                });
            } catch (e) {
                if (timedOut) {
                    finish({ok: false, exitCode: -1, stdout: '', stderr: '',
                        timedOut: true, error: `timeout after ${timeoutSec}s`});
                } else if (stdinText !== null && e instanceof GLib.Error &&
                        e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.BROKEN_PIPE)) {
                    // The binary died before draining the pipe: reading its
                    // patch failed, not the spawn itself.
                    finish({ok: false, exitCode: -1, stdout: '',
                        stderr: 'binary exited before reading the patch',
                        timedOut: false, error: 'binary exited before reading the patch'});
                } else if (e instanceof GLib.Error &&
                        e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) {
                    finish({ok: false, exitCode: -1, stdout: '', stderr: '',
                        timedOut: false, error: 'cancelled'});
                } else {
                    finish({ok: false, exitCode: -1, stdout: '', stderr: String(e),
                        timedOut: false, error: String(e)});
                }
            }
        });
    });
}

/**
 * Run `argv` and collect its output. Resolves — NEVER rejects — with
 * {ok, exitCode, stdout, stderr, timedOut, error}: `ok` is true only for a
 * clean exit 0; spawn failures, cancellations, and timeouts all resolve
 * with `ok:false` and a human-readable `error` (`timedOut:true` on a
 * timeout, after the process was force-exited). `timeoutSec` defaults to
 * 60.
 */
export function runJson(argv, {timeoutSec = 60, cancellable = null} = {}) {
    return _collect(argv, null, timeoutSec, cancellable);
}

/**
 * Run `argv` feeding `stdinText` over stdin — the ONLY sanctioned transport
 * for `settings apply` payloads (never argv). Resolves — NEVER rejects —
 * with the same result shape as runJson (`timeoutSec` defaults to 30). A
 * null or undefined `stdinText` never spawns: it resolves as an internal
 * error, keeping the never-reject contract while refusing the bad call. A
 * binary that exits before draining stdin maps Gio's BROKEN_PIPE to the
 * readable "binary exited before reading the patch" error.
 */
export function runWithStdin(argv, stdinText, {timeoutSec = 30, cancellable = null} = {}) {
    if (stdinText === null || stdinText === undefined)
        return Promise.resolve({ok: false, exitCode: -1, stdout: '', stderr: '',
            timedOut: false, error: 'internal error: stdin payload missing'});
    return _collect(argv, String(stdinText), timeoutSec, cancellable);
}
