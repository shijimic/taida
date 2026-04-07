//! `taida-addon-terminal-sample` — minimal terminal addon for RC1.5.
//!
//! This is the **proof-of-concept addon** that exercises the install-time
//! prebuild pipeline (RC1.5). It exposes exactly 5 sync functions over
//! the frozen ABI v1:
//!
//! - `termPrint(s: Str) -> Unit` — print a string to stdout (no newline)
//! - `termPrintLn(s: Str) -> Unit` — print a string to stdout + newline
//! - `termReadLine() -> Str` — read one line from stdin
//! - `termSize() -> @(rows: Int, cols: Int)` — terminal window dimensions
//! - `termIsTty() -> Bool` — whether stdin is a TTY
//!
//! This crate is `#[cfg(unix)]` only — non-unix targets must see a
//! compile-time error (design lock). Both `rlib` (cargo test) and
//! `cdylib` (native addon loader) outputs are produced.

#![cfg(unix)]

use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicPtr, Ordering};

use taida_addon::bridge::HostValueBuilder;
use taida_addon::{
    TaidaAddonErrorV1, TaidaAddonFunctionV1, TaidaAddonStatus, TaidaAddonValueV1, TaidaHostV1,
};

/// Captured host callback table. Populated by `terminal_init` and read
/// by per-call entry points.
static HOST_PTR: AtomicPtr<TaidaHostV1> = AtomicPtr::new(core::ptr::null_mut());

extern "C" fn terminal_init(host: *const TaidaHostV1) -> TaidaAddonStatus {
    if host.is_null() {
        return TaidaAddonStatus::NullPointer;
    }
    HOST_PTR.store(host as *mut _, Ordering::Release);
    TaidaAddonStatus::Ok
}

// ── Helpers ──────────────────────────────────────────────────────

/// Get the host builder from the captured init pointer.
fn get_builder() -> Option<(HostValueBuilder<'static>, *const TaidaHostV1)> {
    let host_ptr = HOST_PTR.load(Ordering::Acquire);
    if host_ptr.is_null() {
        return None;
    }
    // SAFETY: host_ptr is captured during init and valid for the addon lifetime.
    let builder = unsafe { HostValueBuilder::from_raw(host_ptr) }?;
    Some((builder, host_ptr))
}

// ── termPrint ────────────────────────────────────────────────────

extern "C" fn term_print(
    args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 1 {
        return TaidaAddonStatus::ArityMismatch;
    }
    let (builder, _host_ptr) = match get_builder() {
        Some(v) => v,
        None => return TaidaAddonStatus::InvalidState,
    };
    // SAFETY: host contract — args_ptr valid for the call.
    let arg = match unsafe { taida_addon::bridge::borrow_arg(args_ptr, args_len, 0) } {
        Some(v) => v,
        None => return TaidaAddonStatus::NullPointer,
    };
    let text = match arg.as_str() {
        Some(s) => s.to_string(),
        None => {
            let err = builder.error(1, "termPrint: argument must be a string");
            if !out_error.is_null() {
                unsafe { *out_error = err };
            }
            return TaidaAddonStatus::Error;
        }
    };

    // Print to stdout (no newline)
    let result = unsafe {
        libc::write(
            libc::STDOUT_FILENO,
            text.as_ptr() as *const c_void,
            text.len(),
        )
    };
    if result < 0 {
        let err = builder.error(2, "termPrint: write to stdout failed");
        if !out_error.is_null() {
            unsafe { *out_error = err };
        }
        return TaidaAddonStatus::Error;
    }

    if !out_value.is_null() {
        unsafe { *out_value = builder.unit() };
    }
    TaidaAddonStatus::Ok
}

// ── termPrintLn ──────────────────────────────────────────────────

extern "C" fn term_print_ln(
    args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 1 {
        return TaidaAddonStatus::ArityMismatch;
    }
    let (builder, _) = match get_builder() {
        Some(v) => v,
        None => return TaidaAddonStatus::InvalidState,
    };
    let arg = match unsafe { taida_addon::bridge::borrow_arg(args_ptr, args_len, 0) } {
        Some(v) => v,
        None => return TaidaAddonStatus::NullPointer,
    };
    let text = match arg.as_str() {
        Some(s) => s,
        None => {
            let err = builder.error(1, "termPrintLn: argument must be a string");
            if !out_error.is_null() {
                unsafe { *out_error = err };
            }
            return TaidaAddonStatus::Error;
        }
    };

    let mut buf = String::with_capacity(text.len() + 1);
    buf.push_str(text);
    buf.push('\n');
    let result = unsafe {
        libc::write(
            libc::STDOUT_FILENO,
            buf.as_ptr() as *const c_void,
            buf.len(),
        )
    };
    if result < 0 {
        let err = builder.error(2, "termPrintLn: write to stdout failed");
        if !out_error.is_null() {
            unsafe { *out_error = err };
        }
        return TaidaAddonStatus::Error;
    }

    if !out_value.is_null() {
        unsafe { *out_value = builder.unit() };
    }
    TaidaAddonStatus::Ok
}

// ── termReadLine ─────────────────────────────────────────────────

extern "C" fn term_read_line(
    _args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 0 {
        return TaidaAddonStatus::ArityMismatch;
    }
    let (builder, _) = match get_builder() {
        Some(v) => v,
        None => return TaidaAddonStatus::InvalidState,
    };

    let mut buf = String::new();
    match std::io::stdin().read_line(&mut buf) {
        Ok(0) => {
            // EOF — error per design lock
            let err = builder.error(3, "termReadLine: EOF on stdin");
            if !out_error.is_null() {
                unsafe { *out_error = err };
            }
            TaidaAddonStatus::Error
        }
        Ok(_) => {
            // Strip trailing newline
            if buf.ends_with('\n') {
                buf.pop();
            }
            if buf.ends_with('\r') {
                buf.pop();
            }
            if !out_value.is_null() {
                unsafe { *out_value = builder.str(&buf) };
            }
            TaidaAddonStatus::Ok
        }
        Err(e) => {
            let err = builder.error(4, &format!("termReadLine: read error: {}", e));
            if !out_error.is_null() {
                unsafe { *out_error = err };
            }
            TaidaAddonStatus::Error
        }
    }
}

// ── termSize ─────────────────────────────────────────────────────

extern "C" fn term_size(
    _args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 0 {
        return TaidaAddonStatus::ArityMismatch;
    }
    let (builder, _) = match get_builder() {
        Some(v) => v,
        None => return TaidaAddonStatus::InvalidState,
    };

    // SAFETY: winsz is stack-allocated and properly initialized
    let mut winsz = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ret = unsafe {
        libc::ioctl(
            libc::STDOUT_FILENO,
            libc::TIOCGWINSZ as _,
            &mut winsz as *mut _,
        )
    };
    if ret < 0 {
        let err = builder.error(5, "termSize: ioctl(TIOCGWINSZ) failed");
        if !out_error.is_null() {
            unsafe { *out_error = err };
        }
        return TaidaAddonStatus::Error;
    }

    let rows_val = winsz.ws_row as i64;
    let cols_val = winsz.ws_col as i64;

    // Build Pack @(rows: Int, cols: Int)
    let rows_name = std::ffi::CString::new("rows").unwrap();
    let cols_name = std::ffi::CString::new("cols").unwrap();

    let host = builder.as_raw();
    let rows_v = unsafe { ((*host).value_new_int)(host, rows_val) };
    let cols_v = unsafe { ((*host).value_new_int)(host, cols_val) };
    if rows_v.is_null() || cols_v.is_null() {
        if !rows_v.is_null() {
            unsafe { ((*host).value_release)(host, rows_v) };
        }
        if !cols_v.is_null() {
            unsafe { ((*host).value_release)(host, cols_v) };
        }
        let err = builder.error(6, "termSize: failed to build pack values");
        if !out_error.is_null() {
            unsafe { *out_error = err };
        }
        return TaidaAddonStatus::Error;
    }

    let names: [*const c_char; 2] = [rows_name.as_ptr(), cols_name.as_ptr()];
    let values: [*mut TaidaAddonValueV1; 2] = [rows_v, cols_v];
    let pack = builder.pack(&names, &values);

    if !out_value.is_null() {
        unsafe { *out_value = pack };
    }
    TaidaAddonStatus::Ok
}

// ── termIsTty ────────────────────────────────────────────────────

extern "C" fn term_is_tty(
    _args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    _out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 0 {
        return TaidaAddonStatus::ArityMismatch;
    }
    let (builder, _) = match get_builder() {
        Some(v) => v,
        None => return TaidaAddonStatus::InvalidState,
    };

    let result = unsafe { libc::isatty(libc::STDIN_FILENO) };
    let is_tty = result == 1;

    if !out_value.is_null() {
        unsafe { *out_value = builder.bool(is_tty) };
    }
    TaidaAddonStatus::Ok
}

// ── Function table ───────────────────────────────────────────────

/// Function table for the terminal sample addon.
pub static TERMINAL_FUNCTIONS: &[TaidaAddonFunctionV1] = &[
    TaidaAddonFunctionV1 {
        name: c"termPrint".as_ptr() as *const c_char,
        arity: 1,
        call: term_print,
    },
    TaidaAddonFunctionV1 {
        name: c"termPrintLn".as_ptr() as *const c_char,
        arity: 1,
        call: term_print_ln,
    },
    TaidaAddonFunctionV1 {
        name: c"termReadLine".as_ptr() as *const c_char,
        arity: 0,
        call: term_read_line,
    },
    TaidaAddonFunctionV1 {
        name: c"termSize".as_ptr() as *const c_char,
        arity: 0,
        call: term_size,
    },
    TaidaAddonFunctionV1 {
        name: c"termIsTty".as_ptr() as *const c_char,
        arity: 0,
        call: term_is_tty,
    },
];

taida_addon::declare_addon! {
    name: "taida-lang/terminal",
    functions: TERMINAL_FUNCTIONS,
    init: terminal_init,
}

// ── Unit tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;
    use taida_addon::{TAIDA_ADDON_ABI_VERSION, TaidaAddonDescriptorV1};

    unsafe extern "C" {
        fn taida_addon_get_v1() -> *const TaidaAddonDescriptorV1;
    }

    #[test]
    fn entry_symbol_returns_descriptor() {
        let ptr = unsafe { taida_addon_get_v1() };
        assert!(!ptr.is_null());
        let d = unsafe { &*ptr };
        assert_eq!(d.abi_version, TAIDA_ADDON_ABI_VERSION);
    }

    #[test]
    fn descriptor_advertises_five_functions() {
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        assert_eq!(d.function_count as usize, TERMINAL_FUNCTIONS.len());
        assert_eq!(d.function_count, 5);
    }

    #[test]
    fn descriptor_addon_name_is_terminal() {
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        let name = unsafe { CStr::from_ptr(d.addon_name) };
        assert_eq!(name.to_str().unwrap(), "taida-lang/terminal");
    }

    #[test]
    fn descriptor_init_is_wired() {
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        assert!(d.init.is_some());
    }

    #[test]
    fn function_table_has_all_five() {
        let expected: Vec<(String, u32)> = vec![
            ("termPrint".to_string(), 1u32),
            ("termPrintLn".to_string(), 1),
            ("termReadLine".to_string(), 0),
            ("termSize".to_string(), 0),
            ("termIsTty".to_string(), 0),
        ];
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        let mut seen = Vec::new();
        for i in 0..d.function_count as isize {
            let f = unsafe { &*d.functions.offset(i) };
            let name = unsafe { CStr::from_ptr(f.name) }.to_str().unwrap();
            seen.push((name.to_string(), f.arity));
        }
        assert_eq!(seen, expected);
    }

    #[test]
    fn term_print_zero_args_is_arity_mismatch() {
        let f = &TERMINAL_FUNCTIONS[0];
        let status = (f.call)(
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }

    #[test]
    fn term_print_ln_zero_args_is_arity_mismatch() {
        let f = &TERMINAL_FUNCTIONS[1];
        let status = (f.call)(
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }

    #[test]
    fn term_read_line_with_args_is_arity_mismatch() {
        let f = &TERMINAL_FUNCTIONS[2];
        // Arity check happens before pointer dereference, so null is safe.
        let status = (f.call)(
            core::ptr::null(),
            1,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }

    #[test]
    fn term_size_with_args_is_arity_mismatch() {
        let f = &TERMINAL_FUNCTIONS[3];
        let status = (f.call)(
            core::ptr::null(),
            1,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }

    #[test]
    fn term_is_tty_with_args_is_arity_mismatch() {
        let f = &TERMINAL_FUNCTIONS[4];
        let status = (f.call)(
            core::ptr::null(),
            1,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }
}
