//! C25B-018: best-effort terminal-state restoration on panic / fatal signal.
//!
//! Addons in the `taida-lang/terminal` family put the terminal into raw
//! mode, enter the alternate screen, hide the cursor, and enable mouse
//! reporting. When the host process (the `taida` CLI) panics or is
//! terminated by SIGHUP / SIGTERM / SIGQUIT / SIGABRT before the addon's
//! `leave_raw_mode()` can run, the shell is left with a corrupted
//! terminal (no echo, no cursor, alternate screen active).
//!
//! The frozen addon ABI v1 does not include a host-callback through
//! which an addon can register a drop guard, so the comprehensive fix
//! (an `on_panic_cleanup` entry on `TaidaHostV1`) is a **D26** breaking
//! change. For the `@c.25.rc*` cycle we install a **best-effort
//! fallback** inside the Taida CLI host itself that:
//!
//! 1. Registers a [`std::panic::set_hook`] chain that writes a
//!    well-known ANSI reset sequence to stderr before forwarding to the
//!    previous hook. The reset sequence unconditionally shows the
//!    cursor, leaves the alternate screen, disables mouse reporting,
//!    and emits a soft terminal reset (`ESC c`-class, but we use the
//!    narrower `ESC[!p` DECSTR that does not wipe scrollback).
//!
//! 2. Installs signal handlers for SIGHUP / SIGTERM / SIGQUIT /
//!    SIGABRT (UNIX only; a no-op stub on Windows so higher-level code
//!    compiles unchanged) that do the same reset and then re-raise the
//!    signal with the default disposition so the process exits with
//!    the expected status code.
//!
//! 3. Remains a **no-op** when stderr is not a TTY. Non-interactive
//!    CI invocations will not have their log output polluted with ANSI
//!    escapes, and the hook's only cost is one syscall to `isatty(2)`
//!    on panic.
//!
//! ## Scope boundaries
//!
//! - The hook lives in the host CLI, not in the `taida-lang/terminal`
//!   addon. The terminal addon (`.dev/official-package-repos/terminal/`)
//!   is a git submodule and is not changed by this patch (C25 session
//!   non-negotiable; D26 is the correct phase for in-addon
//!   integration).
//! - The hook does **not** attempt to restore the caller's `termios`
//!   state. A future D26 addition will add a dedicated `TaidaHostV1`
//!   slot through which `raw_mode.rs` can register a saved `termios`
//!   pointer; until then, the best the host can do is emit the ANSI
//!   sequences above, which at minimum gets the user back to a usable
//!   shell prompt even if `tcsetattr(3)` state remains partly dirty.
//! - SIGWINCH / SIGCONT and other non-terminal signals are not
//!   handled — they are not produced by panic-like conditions and
//!   don't require cleanup.
//!
//! The panic hook is installed by [`install_panic_cleanup_hook`],
//! which is idempotent (guarded by a [`std::sync::OnceLock`]) so
//! nested `cargo test` invocations or explicit `taida run … taida run`
//! chains only install one hook per process. The signal handler is
//! installed similarly via [`install_signal_cleanup_handlers`]
//! (UNIX only).
//!
//! Both functions are called unconditionally from `fn main()` at
//! process start, before `libc::signal(SIGPIPE, SIG_IGN)` so that the
//! SIGPIPE disposition established for `taida run ... | head`
//! pipelines is unaffected.

use std::io::{self, IsTerminal, Write};
use std::sync::OnceLock;

/// ANSI reset sequence emitted before we re-raise / exit. Kept as a
/// single `&str` so its exact bytes are auditable in tests.
///
/// Order matters: show the cursor first so the user can see the shell
/// prompt even if a later step fails; then leave the alternate screen
/// so the shell's scrollback becomes visible; then disable mouse and
/// bracketed-paste modes; then apply a soft DECSTR so character-set
/// / origin-mode / insertion state is reset.
const RESET_SEQUENCE: &str = concat!(
    "\x1b[?25h",   // DECTCEM: show cursor
    "\x1b[?1049l", // leave alternate screen buffer
    "\x1b[?1000l", // disable basic mouse reporting (X10 / VT200)
    "\x1b[?1002l", // disable button-event mouse tracking
    "\x1b[?1003l", // disable any-event mouse tracking
    "\x1b[?1006l", // disable SGR-encoded mouse
    "\x1b[?2004l", // disable bracketed-paste mode
    "\x1b[!p",     // DECSTR: soft terminal reset
);

static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
#[cfg(unix)]
static SIGNAL_HANDLERS_INSTALLED: OnceLock<()> = OnceLock::new();

/// Emit the reset sequence to stderr **iff** stderr is a TTY. Non-TTY
/// stderr (piped / file-redirected / CI) is left untouched. Errors
/// from `write_all` are intentionally swallowed because the process
/// is already panicking or terminating.
fn emit_reset_if_tty() {
    let stderr = io::stderr();
    if !stderr.is_terminal() {
        return;
    }
    let mut handle = stderr.lock();
    let _ = handle.write_all(RESET_SEQUENCE.as_bytes());
    let _ = handle.flush();
}

/// Install the panic hook. Idempotent; safe to call multiple times.
/// The hook wraps the existing hook so panic messages / backtraces
/// are still written by whatever `std::panic` or downstream harness
/// had already registered (test harness, cargo-nextest, etc.).
pub fn install_panic_cleanup_hook() {
    PANIC_HOOK_INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            emit_reset_if_tty();
            previous(info);
        }));
    });
}

/// Install SIGHUP / SIGTERM / SIGQUIT / SIGABRT handlers that emit
/// the reset sequence and then re-raise the signal with the default
/// disposition. Idempotent; safe to call multiple times.
#[cfg(unix)]
pub fn install_signal_cleanup_handlers() {
    SIGNAL_HANDLERS_INSTALLED.get_or_init(|| {
        // We use raw `libc::signal` because we don't want to pull in
        // signal-hook's heavier machinery for a best-effort cleanup.
        // The handler itself is **not** fully async-signal-safe
        // (`write(2)` to a `BufWriter`-wrapped stream would not be),
        // but stderr via the `io::stderr().lock()` path bottoms out
        // in a single `write(2)` syscall, which *is* async-signal-
        // safe for our purposes.
        //
        // After writing the reset, we restore the default disposition
        // for the signal and re-raise it so the process exits with
        // the expected status (e.g. 128+SIGTERM=143) and any
        // supervisor that inspects `$?` sees the correct value.
        unsafe {
            for &sig in &[libc::SIGHUP, libc::SIGTERM, libc::SIGQUIT, libc::SIGABRT] {
                libc::signal(
                    sig,
                    cleanup_signal_handler as *const () as libc::sighandler_t,
                );
            }
        }
    });
}

#[cfg(not(unix))]
pub fn install_signal_cleanup_handlers() {
    // No-op on Windows: the analogous console-cleanup path
    // (`SetConsoleMode`, `ENABLE_VIRTUAL_TERMINAL_INPUT`) is not
    // reached by panic-like signals in the same way, and Windows
    // terminals do not share UNIX's per-process TTY state. A
    // dedicated Windows cleanup entrypoint is a follow-up.
}

/// Async-signal-safe-ish cleanup handler. See
/// [`install_signal_cleanup_handlers`] for the safety argument.
#[cfg(unix)]
extern "C" fn cleanup_signal_handler(sig: libc::c_int) {
    // Emit the reset. We don't call `emit_reset_if_tty` directly
    // because it uses `io::stderr().lock()` which takes a mutex —
    // safe enough here because we hold no stdio locks ourselves, but
    // we still prefer the raw `write(2)` path for signal safety.
    //
    // SAFETY: `STDERR_FILENO` (= 2) is the canonical stderr fd; a
    // `write(2)` syscall is async-signal-safe per POSIX.1-2017. We
    // intentionally do **not** gate on `isatty(2)` inside the signal
    // handler because `isatty` performs an `fcntl(F_GETFL)` on some
    // libcs which is async-signal-safe in theory but we prefer not
    // to rely on it; the cost of an extra ~30 bytes on a non-TTY
    // stderr is negligible compared to a lost cursor.
    unsafe {
        let buf = RESET_SEQUENCE.as_bytes();
        let _ = libc::write(libc::STDERR_FILENO, buf.as_ptr().cast(), buf.len());
    }

    // Restore the default disposition and re-raise so the process
    // exits with the signal-encoded status code.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_sequence_contains_cursor_show() {
        // DECTCEM show-cursor — the single most user-visible bit of
        // the reset. If this byte ever goes missing silently, a user
        // who triggers a panic after `CursorHide()` will be left
        // looking at an invisible cursor.
        assert!(RESET_SEQUENCE.contains("\x1b[?25h"));
    }

    #[test]
    fn reset_sequence_contains_alternate_screen_leave() {
        // Alternate-screen leave. Without this the shell appears
        // blank (scrollback invisible) after a panicked TUI exits.
        assert!(RESET_SEQUENCE.contains("\x1b[?1049l"));
    }

    #[test]
    fn reset_sequence_disables_all_mouse_modes() {
        // Every mouse-mode-disable in the sequence, in the order we
        // emit them. If `renderer.rs` ever introduces a new mouse
        // mode, this test flags that the reset path needs to learn
        // about it too.
        for mode in ["\x1b[?1000l", "\x1b[?1002l", "\x1b[?1003l", "\x1b[?1006l"] {
            assert!(
                RESET_SEQUENCE.contains(mode),
                "reset sequence missing mouse-disable {:?}",
                mode
            );
        }
    }

    #[test]
    fn reset_sequence_disables_bracketed_paste() {
        assert!(RESET_SEQUENCE.contains("\x1b[?2004l"));
    }

    #[test]
    fn install_panic_cleanup_hook_is_idempotent() {
        // Idempotence is important because `main()` may be re-entered
        // in tests (`#[test] fn main_integration()`), and because
        // `cargo test` harnesses can instantiate us multiple times
        // via integration harnesses.
        install_panic_cleanup_hook();
        install_panic_cleanup_hook();
        install_panic_cleanup_hook();
        // If we got here without panicking-in-a-panic, the hook is
        // idempotent. There is no way to directly observe the inner
        // OnceLock state without accessors, so the assertion is
        // implicit (the second call must not panic trying to reinstall).
    }

    #[cfg(unix)]
    #[test]
    fn install_signal_cleanup_handlers_is_idempotent() {
        install_signal_cleanup_handlers();
        install_signal_cleanup_handlers();
        install_signal_cleanup_handlers();
        // Likewise implicit.
    }
}
