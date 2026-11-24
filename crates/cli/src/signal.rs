//! Ctrl-C: during a turn it asks the turn to stop; at the prompt it ends the program.
//!
//! The crate has no signal dependency, so the handler is installed through the C library's
//! own `signal`, which every Rust program on Unix links anyway.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the handler; the turn loop takes it and interrupts the turn.
static PRESSED: AtomicBool = AtomicBool::new(false);
/// Whether a turn is running: Ctrl-C outside one exits.
static IN_TURN: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
mod sys {
    /// `SIGINT` on Linux and macOS.
    pub const SIGINT: i32 = 2;

    unsafe extern "C" {
        pub fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
        pub fn _exit(status: i32) -> !;
    }
}

#[cfg(unix)]
extern "C" fn on_interrupt(_signum: i32) {
    if IN_TURN.load(Ordering::SeqCst) {
        PRESSED.store(true, Ordering::SeqCst);
    } else {
        // SAFETY: `_exit` is async-signal-safe and takes a plain status code.
        unsafe { sys::_exit(130) }
    }
}

/// Installs the handler. Without it (tests, other platforms) Ctrl-C keeps its default.
pub fn install() {
    #[cfg(unix)]
    // SAFETY: the handler only touches atomics and calls `_exit`, both async-signal-safe.
    unsafe {
        sys::signal(sys::SIGINT, on_interrupt);
    }
}

/// Marks whether a turn is running.
pub fn set_in_turn(on: bool) {
    PRESSED.store(false, Ordering::SeqCst);
    IN_TURN.store(on, Ordering::SeqCst);
}

/// Whether Ctrl-C was pressed since the last look.
pub fn take_pressed() -> bool {
    PRESSED.swap(false, Ordering::SeqCst)
}
