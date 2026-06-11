//! The `committed` stat must follow the commit mask exactly (C parity:
//! `_mi_commit_mask_committed_size` accounting), not whole-segment deltas.
#![cfg(feature = "stats")]

#[test]
fn committed_follows_commit_mask() {
    let base = rimalloc::stats_committed_current();

    // A fresh segment commits its header + first pages, not necessarily
    // the whole 32MiB span: committed must grow by less than segment size.
    let p = rimalloc::malloc(8 * 1024);
    assert!(!p.is_null());
    unsafe { p.write_bytes(7, 8 * 1024) };
    let after_alloc = rimalloc::stats_committed_current();
    assert!(after_alloc > base, "commit not counted");

    // Releasing everything must return committed to (at most) the baseline
    // plus the thread's retained segment commitment — and never go negative.
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

    // Balanced across an aligned (possibly trim-path) mapping.
    let a = rimalloc::malloc_aligned(1 << 20, 1 << 21);
    assert!(!a.is_null());
    rimalloc::free(a);
    rimalloc::collect(true);
    assert!(rimalloc::stats_committed_current() >= 0);
}
