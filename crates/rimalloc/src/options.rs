//! Runtime options (port of `options.c`): each option reads its
//! `MIMALLOC_*` environment variable once and is settable at runtime
//! (`mi_option_get`/`mi_option_set`).

use core::sync::atomic::{AtomicI64, Ordering};

macro_rules! options {
    ($($variant:ident => $name:ident: $default:expr, $env:literal;)*) => {
        /// `mi_option_t`
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        #[repr(usize)]
        pub enum Option {
            $($variant),*
        }

        const COUNT: usize = [$($env),*].len();
        const DEFAULTS: [(i64, &str); COUNT] = [$(($default, $env)),*];
        // i64::MIN marks "not yet initialized from the environment".
        static VALUES: [AtomicI64; COUNT] = [const { AtomicI64::new(i64::MIN) }; COUNT];

        $(
            /// Internal shorthand for this option's value.
            #[allow(dead_code)]
            #[inline]
            pub fn $name() -> i64 {
                get(Option::$variant)
            }
        )*
    };
}

options! {
    EagerCommit => eager_commit: 1, "MIMALLOC_EAGER_COMMIT";
    EagerCommitDelay => eager_commit_delay: 0, "MIMALLOC_EAGER_COMMIT_DELAY";
    PurgeDelay => purge_delay: 10, "MIMALLOC_PURGE_DELAY";
    PurgeExtendDelay => purge_extend_delay: 1, "MIMALLOC_PURGE_EXTEND_DELAY";
    AbandonedPagePurge => abandoned_page_purge: 0, "MIMALLOC_ABANDONED_PAGE_PURGE";
    FullPageRetain => full_page_retain: 2, "MIMALLOC_FULL_PAGE_RETAIN";
    MaxPageCandidates => max_page_candidates: 4, "MIMALLOC_MAX_PAGE_CANDIDATES";
    MaxSegmentReclaim => max_segment_reclaim: 10, "MIMALLOC_MAX_SEGMENT_RECLAIM";
    AbandonedReclaimOnFree => abandoned_reclaim_on_free: 0, "MIMALLOC_ABANDONED_RECLAIM_ON_FREE";
    TargetSegmentsPerThread => target_segments_per_thread: 0, "MIMALLOC_TARGET_SEGMENTS_PER_THREAD";
    ArenaReserve => arena_reserve: 1024 * 1024, "MIMALLOC_ARENA_RESERVE"; // KiB
    ShowErrors => show_errors: 0, "MIMALLOC_SHOW_ERRORS";
    ShowStats => show_stats: 0, "MIMALLOC_SHOW_STATS";
    Verbose => verbose: 0, "MIMALLOC_VERBOSE";
}

fn parse(s: &str) -> std::option::Option<i64> {
    match s.to_ascii_lowercase().as_str() {
        "" | "on" | "yes" | "true" => Some(1),
        "off" | "no" | "false" => Some(0),
        v => v.parse().ok(),
    }
}

/// `mi_option_get`
pub fn get(option: Option) -> i64 {
    let idx = option as usize;
    match VALUES[idx].load(Ordering::Relaxed) {
        i64::MIN => {
            let (default, env) = DEFAULTS[idx];
            let v = std::env::var(env)
                .ok()
                .and_then(|s| parse(&s))
                .unwrap_or(default);
            VALUES[idx].store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// `mi_option_set`
pub fn set(option: Option, value: i64) {
    VALUES[option as usize].store(value, Ordering::Relaxed);
}

/// `mi_option_is_enabled`
pub fn is_enabled(option: Option) -> bool {
    get(option) != 0
}

/// `mi_option_enable` / `mi_option_disable`
pub fn enable(option: Option) {
    set(option, 1)
}
pub fn disable(option: Option) {
    set(option, 0)
}

/// `_mi_clock_now`: monotonic milliseconds. Only used for relative purge
/// deadlines (>= 10ms granularity), so on Apple the cheap commpage-cached
/// approximate clock is used; `CLOCK_MONOTONIC` there is a gettimeofday +
/// boottime computation that shows up in thread-churn profiles.
#[cfg(all(target_vendor = "apple", not(miri)))]
pub fn clock_now() -> i64 {
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
    }
    // SAFETY: no preconditions.
    (unsafe { clock_gettime_nsec_np(libc::CLOCK_MONOTONIC_RAW_APPROX) } / 1_000_000) as i64
}

/// `_mi_clock_now`: monotonic milliseconds.
#[cfg(not(all(target_vendor = "apple", not(miri))))]
#[allow(clippy::unnecessary_cast)] // tv_sec/tv_nsec are narrower off-macOS
pub fn clock_now() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1000 + ts.tv_nsec as i64 / 1_000_000
}
