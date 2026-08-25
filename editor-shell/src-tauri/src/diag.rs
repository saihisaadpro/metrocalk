//! The diagnostic channel a windows-subsystem GUI binary does not otherwise have.
//!
//! The release editor is built `#![windows_subsystem = "windows"]`, so the process starts with **no
//! console attached**: `stderr` is an invalid handle and every `eprintln!` in this crate — including
//! the message the Rust runtime prints when a thread panics — is written nowhere. That is the whole
//! explanation for the class of failure where the editor terminates mid-session leaving no stderr, no
//! panic text and no Windows Error Reporting entry: from outside the process, a clean unwind out of
//! `main`, a webview teardown and a healthy user-initiated exit are literally indistinguishable.
//!
//! So the process is given a channel it owns: an append-only log beside the executable, opened and
//! flushed per line (never held open, so a hard termination cannot lose buffered lines), carrying
//!
//! * a session banner with the pid, exe and build profile,
//! * every panic — thread, payload, source location and backtrace — via a `set_hook` that then
//!   delegates to the previous hook so ordinary behaviour is unchanged,
//! * the last-chance Win32 exception filter, which is the only way a stack overflow or an access
//!   violation in a driver leaves a trace of its own,
//! * lifecycle breadcrumbs from the tao event loop (window destroyed, exit requested, exit), which
//!   is what distinguishes "the app was asked to close" from "the app fell over",
//! * and whatever the render loop and command loop choose to record through [`log`].
//!
//! Reading it is the first step in diagnosing any unexplained exit: `metrocalk-diagnostics.log`
//! beside `metrocalk-editor-shell.exe`. A run that ends with `lifecycle: Exit` closed normally; a run
//! whose last line is anything else did not.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The diagnostic log's path: beside the executable, like every other sidecar this app keeps, so it
/// is stable across launches of the same build and trivially findable from a harness.
pub fn path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("metrocalk-diagnostics.log")
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` from the wall clock, so a line can be lined up against an e2e
/// transcript. Hand-rolled (civil-from-days) rather than pulling a date crate into the app for one
/// format string.
fn stamp() -> String {
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d,
        Err(_) => return "0000-00-00T00:00:00.000Z".into(),
    };
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days, shifted to a 0000-03-01 era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Append one diagnostic line. Opens, writes and closes every time: a few microseconds against the
/// certainty that nothing is sitting in a buffer when the process dies, which is the exact case this
/// file exists to explain. Never panics and never fails a caller — a diagnostic that can take the app
/// down would be worse than no diagnostic at all.
pub fn log(message: &str) {
    let line = format!("{} [{}] {message}\n", stamp(), std::process::id());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path())
    {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
    // Still useful under `cargo run` / the debug build, which does have a console.
    eprint!("[shell] {line}");
}

/// `log` with `format!` ergonomics.
#[macro_export]
macro_rules! diag_log {
    ($($arg:tt)*) => { $crate::diag::log(&format!($($arg)*)) };
}

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Install the panic hook and the last-chance exception filter, and open the session. Idempotent.
pub fn init() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    log(&format!(
        "=== session start · exe={:?} · profile={} · diagnostics={:?}",
        std::env::current_exe().unwrap_or_default(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        path()
    ));

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>").to_string();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".into());
        let where_ = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".into());
        log(&format!(
            "PANIC on thread '{name}' at {where_}: {payload}\n{}",
            std::backtrace::Backtrace::force_capture()
        ));
        previous(info);
    }));

    #[cfg(windows)]
    install_last_chance_filter();
}

// ── Win32 last-chance exception filter ────────────────────────────────────────────────────────────
// A Rust panic unwinds and the hook above sees it. A *hardware* fault — a stack overflow from an
// unbounded recursion, an access violation inside a graphics driver — does not: the process is torn
// down by the OS, and with WER silenced for this binary class there is nothing left behind at all.
// `SetUnhandledExceptionFilter` is called once, after every handler in the chain has declined, and is
// the last code of ours that will ever run. It is declared here by hand rather than adding a Win32
// crate to the app for four symbols.
#[cfg(windows)]
mod win32 {
    #[repr(C)]
    pub struct ExceptionRecord {
        pub code: u32,
        pub flags: u32,
        pub record: *mut ExceptionRecord,
        pub address: *mut core::ffi::c_void,
        pub number_parameters: u32,
        pub information: [usize; 15],
    }

    #[repr(C)]
    pub struct ExceptionPointers {
        pub exception_record: *mut ExceptionRecord,
        pub context_record: *mut core::ffi::c_void,
    }

    pub type TopLevelFilter =
        Option<unsafe extern "system" fn(*mut ExceptionPointers) -> core::ffi::c_long>;

    extern "system" {
        pub fn SetUnhandledExceptionFilter(filter: TopLevelFilter) -> TopLevelFilter;
    }

    /// `EXCEPTION_CONTINUE_SEARCH` — decline, so the OS default (terminate, and WER if enabled) still
    /// happens exactly as before. This filter only ever observes; it never changes the outcome.
    pub const CONTINUE_SEARCH: core::ffi::c_long = 0;
}

#[cfg(windows)]
fn install_last_chance_filter() {
    // Safety: a plain Win32 registration; the callback is a `extern "system" fn` with the documented
    // signature and does not unwind (everything inside is `let _ =`-swallowed I/O).
    unsafe {
        win32::SetUnhandledExceptionFilter(Some(last_chance));
    }
}

#[cfg(windows)]
unsafe extern "system" fn last_chance(
    pointers: *mut win32::ExceptionPointers,
) -> core::ffi::c_long {
    let (code, address) = if pointers.is_null() {
        (0, std::ptr::null_mut())
    } else {
        let record = (*pointers).exception_record;
        if record.is_null() {
            (0, std::ptr::null_mut())
        } else {
            ((*record).code, (*record).address)
        }
    };
    // Named for the two that actually matter here: an unbounded recursion over a large imported
    // assembly, and a driver fault under the wgpu path.
    let name = match code {
        0xC000_0005 => "EXCEPTION_ACCESS_VIOLATION",
        0xC000_00FD => "STACK_OVERFLOW",
        0xC000_001D => "ILLEGAL_INSTRUCTION",
        0xC000_0094 => "INT_DIVIDE_BY_ZERO",
        0xC000_0374 => "HEAP_CORRUPTION",
        0x8000_0003 => "BREAKPOINT",
        _ => "unhandled exception",
    };
    log(&format!(
        "FATAL {name} (0x{code:08X}) at {address:p} on thread '{}' — the process is being terminated \
         by the OS. This is a hardware/OS fault, not a Rust panic.",
        std::thread::current().name().unwrap_or("<unnamed>")
    ));
    win32::CONTINUE_SEARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_a_sortable_utc_instant() {
        let s = stamp();
        assert_eq!(s.len(), 24, "{s}");
        assert!(s.ends_with('Z'), "{s}");
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
        // A plausible year — this is the check that would catch a broken civil-from-days.
        let year: i64 = s[0..4].parse().expect("year");
        assert!((2020..2200).contains(&year), "{s}");
    }

    #[test]
    fn the_log_path_sits_beside_the_executable() {
        let p = path();
        assert_eq!(
            p.file_name().and_then(|n| n.to_str()),
            Some("metrocalk-diagnostics.log")
        );
        assert_eq!(
            p.parent(),
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
                .as_deref()
        );
    }

    #[test]
    fn init_is_idempotent_and_writing_never_panics() {
        init();
        init();
        log("self-test: the diagnostic channel is open");
    }
}
