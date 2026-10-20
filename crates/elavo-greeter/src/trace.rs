//! Startup timing. `ELAVO_GREETER_TRACE=1` prints each step's time on
//! stderr, counted from when the first `elavo-greeter` process started, so a
//! `--lock` child counts from its parent's start.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const START_VAR: &str = "ELAVO_GREETER_TRACE_START";

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn start() -> Option<u128> {
    static START: OnceLock<Option<u128>> = OnceLock::new();
    *START.get_or_init(|| {
        std::env::var_os("ELAVO_GREETER_TRACE")?;
        let inherited = std::env::var(START_VAR).ok().and_then(|s| s.parse().ok());
        let start = inherited.unwrap_or_else(now_ms);
        // Runs first in main, before any other thread starts. A `--lock`
        // child inherits it.
        std::env::set_var(START_VAR, start.to_string());
        Some(start)
    })
}

/// Call once at the top of `main`.
pub fn init() {
    start();
}

pub fn mark(step: &str) {
    if let Some(start) = start() {
        eprintln!("elavo-greeter: {:>5} ms  {step}", now_ms() - start);
    }
}
