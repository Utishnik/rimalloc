//! Runtime options (subset of `options.c`), read from the environment once.

use core::sync::atomic::{AtomicI64, Ordering};

macro_rules! options {
    ($($name:ident: $default:expr, $env:literal;)*) => {
        $(#[allow(dead_code)]
        pub fn $name() -> i64 {
            static V: AtomicI64 = AtomicI64::new(i64::MIN);
            match V.load(Ordering::Relaxed) {
                i64::MIN => {
                    let v = std::env::var($env).ok()
                        .and_then(|s| parse(&s))
                        .unwrap_or($default);
                    V.store(v, Ordering::Relaxed);
                    v
                }
                v => v,
            }
        })*
    };
}

fn parse(s: &str) -> Option<i64> {
    match s.to_ascii_lowercase().as_str() {
        "" | "on" | "yes" | "true" => Some(1),
        "off" | "no" | "false" => Some(0),
        v => v.parse().ok(),
    }
}

#[allow(dead_code)]
mod unused {}

options! {
    eager_commit: 1, "MIMALLOC_EAGER_COMMIT";
    eager_commit_delay: 0, "MIMALLOC_EAGER_COMMIT_DELAY";
    purge_delay: 10, "MIMALLOC_PURGE_DELAY";
    purge_extend_delay: 1, "MIMALLOC_PURGE_EXTEND_DELAY";
    abandoned_page_purge: 0, "MIMALLOC_ABANDONED_PAGE_PURGE";
    full_page_retain: 2, "MIMALLOC_FULL_PAGE_RETAIN";
    max_page_candidates: 4, "MIMALLOC_MAX_PAGE_CANDIDATES";
    max_segment_reclaim: 10, "MIMALLOC_MAX_SEGMENT_RECLAIM";
    target_segments_per_thread: 0, "MIMALLOC_TARGET_SEGMENTS_PER_THREAD";
}

/// `_mi_clock_now`: monotonic milliseconds.
pub fn clock_now() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
}
