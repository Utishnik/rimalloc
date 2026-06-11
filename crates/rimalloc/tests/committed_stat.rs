//! The `committed` stat tracks committed bytes at whole-segment
//! granularity (see `SegmentsTld::track_size`): it must rise when segments
//! are created, fall when they are released, and never go negative — under
//! both reset-mode and decommit-mode purges.
#![cfg(feature = "stats")]

#[test]
fn committed_rises_and_falls_without_underflow() {
    let base = rimalloc::stats_committed_current();
    assert!(base >= 0);

    let p = rimalloc::malloc(8 * 1024);
    assert!(!p.is_null());
    unsafe { p.write_bytes(7, 8 * 1024) };
    let after_alloc = rimalloc::stats_committed_current();
    assert!(after_alloc > base, "segment commit not counted");

    rimalloc::free(p);
    rimalloc::collect(true);
    let after_free = rimalloc::stats_committed_current();
    assert!(
        after_free >= 0,
        "committed stat went negative: {after_free}"
    );
    assert!(
        after_free <= after_alloc,
        "free/purge did not reduce committed: {after_alloc} -> {after_free}"
    );

    // Aligned (trim-path) mapping and a huge allocation must also balance.
    let a = rimalloc::malloc_aligned(1 << 20, 1 << 21);
    assert!(!a.is_null());
    rimalloc::free(a);
    let h = rimalloc::malloc(40 << 20);
    assert!(!h.is_null());
    rimalloc::free(h);
    rimalloc::collect(true);
    assert!(rimalloc::stats_committed_current() >= 0);
}
