//! Where a render's time goes: with `PL_STAGES` set, each stage's time is printed to
//! stderr (`cargo run -p imaging --example renderbench` then shows a breakdown).

use std::cell::Cell;
use std::sync::LazyLock;
use std::time::Instant;

static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("PL_STAGES").is_some());

thread_local! {
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// A render starts on this thread.
pub(super) fn begin() {
    if *ON {
        LAST.set(Some(Instant::now()));
    }
}

/// The stage `name` just ended: print the time since the last mark.
pub(super) fn mark(name: &str) {
    if *ON {
        let now = Instant::now();
        if let Some(last) = LAST.replace(Some(now)) {
            eprintln!("    {name:22} {:6.1} ms", (now - last).as_secs_f64() * 1000.0);
        }
    }
}
