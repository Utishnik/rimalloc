//! OS memory primitives (port of `os.c` and `prim/unix/prim.c`).
//!
//! Segments need `SEGMENT_ALIGN`-aligned virtual memory; we over-allocate
//! and trim, the same fallback mimalloc uses. Memory can be reserved
//! without commit (`PROT_NONE`) and committed/purged lazily.

use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

#[inline]
pub fn page_size() -> usize {
    static PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);
    match PAGE_SIZE.load(Ordering::Relaxed) {
        0 => {
            // SAFETY: sysconf is async-signal-safe and has no memory preconditions.
            let sz = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
            PAGE_SIZE.store(sz, Ordering::Relaxed);
            sz
        }
        sz => sz,
    }
}

/// An owned range of OS-allocated memory (provenance for a segment).
/// Mirrors the `MI_MEM_OS` case of `mi_memid_t`.
#[derive(Clone, Copy, Debug)]
pub struct OsMem {
    pub base: *mut u8,
    pub size: usize,
    pub align: usize,
    pub is_committed: bool,
    pub is_zero: bool,
}

// SAFETY: OsMem is a plain descriptor; ownership transfer is what Send is for.
unsafe impl Send for OsMem {}

// Under Miri there is no virtual-memory model: partial munmap, PROT_NONE
// reservations, and madvise have no equivalent. Back segments with
// `std::alloc` instead; commit/decommit/reset become no-ops.
#[cfg(miri)]
mod shim {
    use super::OsMem;
    use core::alloc::Layout;

    impl OsMem {
        pub fn alloc_aligned(
            size: usize,
            alignment: usize,
            commit: bool,
        ) -> Option<(*mut u8, OsMem)> {
            let layout = Layout::from_size_align(size, alignment).ok()?;
            // SAFETY: non-zero size and valid layout.
            let p = unsafe { std::alloc::alloc_zeroed(layout) };
            if p.is_null() {
                return None;
            }
            p.expose_provenance(); // segment lookups reconstruct pointers by address
            Some((
                p,
                OsMem {
                    base: p,
                    size,
                    align: alignment,
                    is_committed: commit,
                    is_zero: true,
                },
            ))
        }

        pub fn free(self) {
            // SAFETY: base/size/align are exactly what alloc_aligned used.
            unsafe {
                std::alloc::dealloc(
                    self.base,
                    Layout::from_size_align_unchecked(self.size, self.align),
                )
            };
        }
    }

    pub unsafe fn commit(_p: *mut u8, _size: usize) -> bool {
        false
    }
    pub unsafe fn decommit(_p: *mut u8, _size: usize) {}
    pub unsafe fn reset(_p: *mut u8, _size: usize) {}
}

#[cfg(miri)]
pub use shim::{commit, decommit, reset};

/// Syscall counters: [mmap, munmap, madvise, mprotect].
pub static SYSCALL_COUNTS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

pub fn syscall_counts() -> [usize; 4] {
    SYSCALL_COUNTS.each_ref().map(|c| c.load(Ordering::Relaxed))
}

#[cfg(not(miri))]
fn mmap(hint: *mut u8, size: usize, commit: bool) -> *mut u8 {
    SYSCALL_COUNTS[0].fetch_add(1, Ordering::Relaxed);
    let prot = if commit {
        libc::PROT_READ | libc::PROT_WRITE
    } else {
        libc::PROT_NONE
    };
    let flags = libc::MAP_PRIVATE | libc::MAP_ANON;
    // SAFETY: anonymous mapping; a hint address never forces placement.
    let p = unsafe { libc::mmap(hint.cast(), size, prot, flags, -1, 0) };
    if p == libc::MAP_FAILED {
        ptr::null_mut()
    } else {
        p.cast()
    }
}

#[cfg(not(miri))]
impl OsMem {
    /// Allocate `size` bytes aligned to `alignment` (a power of two).
    /// Returns the aligned pointer and the owning region.
    pub fn alloc_aligned(size: usize, alignment: usize, commit: bool) -> Option<(*mut u8, OsMem)> {
        debug_assert!(crate::constants::is_power_of_two(alignment));
        let size = crate::constants::align_up(size, page_size());
        let p = mmap(ptr::null_mut(), size, commit);
        if p.is_null() {
            return None;
        }
        if commit {
            crate::stats::STATS.committed.increase(size);
        }
        if p.addr() & (alignment - 1) == 0 {
            p.expose_provenance(); // segment lookups reconstruct pointers by address
            return Some((
                p,
                OsMem {
                    base: p,
                    size,
                    align: alignment,
                    is_committed: commit,
                    is_zero: true,
                },
            ));
        }
        // Misaligned: over-allocate and trim both ends.
        if commit {
            crate::stats::STATS.committed.decrease(size);
        }
        // SAFETY: p..p+size is our mapping.
        unsafe { libc::munmap(p.cast(), size) };
        let over = size + alignment;
        let p = mmap(ptr::null_mut(), over, commit);
        if p.is_null() {
            return None;
        }
        if commit {
            crate::stats::STATS.committed.increase(size);
        }
        let aligned =
            ptr::with_exposed_provenance_mut::<u8>((p.addr() + alignment - 1) & !(alignment - 1));
        let pre = aligned.addr() - p.addr();
        let post = over - pre - size;
        // SAFETY: trimming unused head/tail of our own fresh mapping.
        unsafe {
            if pre > 0 {
                libc::munmap(p.cast(), pre);
            }
            if post > 0 {
                libc::munmap(aligned.add(size).cast(), post);
            }
        }
        aligned.expose_provenance(); // segment lookups reconstruct pointers by address
        Some((
            aligned,
            OsMem {
                base: aligned,
                size,
                align: alignment,
                is_committed: commit,
                is_zero: true,
            },
        ))
    }

    /// Unmap the whole region.
    pub fn free(self) {
        SYSCALL_COUNTS[1].fetch_add(1, Ordering::Relaxed);
        // SAFETY: base..base+size is an owned mapping; no references outlive it.
        unsafe { libc::munmap(self.base.cast(), self.size) };
    }
}

/// Commit a range: make it readable/writable. Returns `is_zero`.
///
/// # Safety
/// `p..p+size` must lie within a single [`OsMem`] region.
#[cfg(not(miri))]
pub unsafe fn commit(p: *mut u8, size: usize) -> bool {
    let (p, size) = page_span(p, size, true);
    crate::stats::STATS.committed.increase(size);
    SYSCALL_COUNTS[3].fetch_add(1, Ordering::Relaxed);
    // SAFETY: caller guarantees the range is within an owned mapping.
    unsafe { libc::mprotect(p.cast(), size, libc::PROT_READ | libc::PROT_WRITE) };
    false // conservatively not known-zero (PROT_NONE ranges were never written though)
}

/// Decommit a range: drop its pages and make it inaccessible.
///
/// # Safety
/// `p..p+size` must lie within a single [`OsMem`] region and have no live
/// references.
#[cfg(not(miri))]
pub unsafe fn decommit(p: *mut u8, size: usize) {
    let (p, size) = page_span(p, size, false);
    if size == 0 {
        return;
    }
    crate::stats::STATS.committed.decrease(size);
    // SAFETY: per caller contract; MAP_FIXED re-map keeps the reservation.
    unsafe {
        libc::mmap(
            p.cast(),
            size,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED,
            -1,
            0,
        );
    }
}

/// Purge: release physical pages but keep the range accessible (`MADV_FREE`).
///
/// # Safety
/// `p..p+size` must lie within a single committed [`OsMem`] region whose
/// contents may be discarded.
#[cfg(not(miri))]
pub unsafe fn reset(p: *mut u8, size: usize) {
    let (p, size) = page_span(p, size, false);
    if size == 0 {
        return;
    }
    #[cfg(target_os = "macos")]
    const ADVICE: libc::c_int = libc::MADV_FREE;
    #[cfg(not(target_os = "macos"))]
    const ADVICE: libc::c_int = libc::MADV_DONTNEED;
    SYSCALL_COUNTS[2].fetch_add(1, Ordering::Relaxed);
    // SAFETY: per caller contract.
    unsafe { libc::madvise(p.cast(), size, ADVICE) };
}

/// Round `(p, size)` to OS page boundaries: conservatively outward when
/// `expand`, inward otherwise.
#[cfg(not(miri))]
fn page_span(p: *mut u8, size: usize, expand: bool) -> (*mut u8, usize) {
    let ps = page_size();
    let (start, end) = if expand {
        (p.addr() & !(ps - 1), (p.addr() + size + ps - 1) & !(ps - 1))
    } else {
        (
            (p.addr() + ps - 1) & !(ps - 1),
            (p.addr() + size) & !(ps - 1),
        )
    };
    (p.with_addr(start), end.saturating_sub(start))
}

// ---------------------------------------------------------------------------
// Deferred frees
// ---------------------------------------------------------------------------
// Freeing a segment while `&Segment`/`&Page` arguments are still on the call
// stack would deallocate memory that live (protected) references point into.
// Instead the region is pushed on a pending list — the node is written into
// the dead region itself — and unmapped later from a shallow call frame.

use core::sync::atomic::AtomicPtr;

/// Intrusive node placement for dead regions: at the *tail*, away from the
/// segment/heap headers which may still be covered by live (protected)
/// references up the call stack when the region is queued.
///
/// # Safety
/// The region must be dead and its tail page committed.
pub(crate) unsafe fn tail_node<T>(mem: &OsMem) -> *mut T {
    debug_assert!(size_of::<T>() <= 64 && mem.size >= 128);
    // SAFETY: in bounds, 16-aligned (size is page aligned).
    unsafe { mem.base.add(mem.size - 64).cast() }
}

struct PendingFree {
    next: *mut PendingFree,
    mem: OsMem,
}

static PENDING: AtomicPtr<PendingFree> = AtomicPtr::new(ptr::null_mut());

/// Queue an [`OsMem`] region for deallocation at the next [`drain_deferred`].
pub fn defer_free(mem: OsMem) {
    // SAFETY: dead region; ensure the tail page is committed (lazily
    // committed segments may have an uncommitted tail, and decommit-mode
    // purges may have dropped it). Stat-balanced: the region's committed
    // bytes were already counted out by the caller.
    let node = unsafe {
        let node = tail_node::<PendingFree>(&mem);
        commit(node.cast(), 64);
        crate::stats::STATS.committed.decrease(page_size());
        node
    };
    let mut head = PENDING.load(Ordering::Relaxed);
    loop {
        // SAFETY: the region is dead; we own it until the actual unmap.
        unsafe { node.write(PendingFree { next: head, mem }) };
        match PENDING.compare_exchange_weak(head, node, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => return,
            Err(h) => head = h,
        }
    }
}

/// Release all pending regions. Call only from frames that hold no
/// references into allocator-owned memory regions.
pub fn drain_deferred() {
    cache_purge_expired();
    if PENDING.load(Ordering::Relaxed).is_null() {
        return;
    }
    let mut node = PENDING.swap(ptr::null_mut(), Ordering::Acquire);
    while !node.is_null() {
        // SAFETY: nodes are exclusively ours after the swap.
        let PendingFree { next, mem } = unsafe { node.read() };
        mem.free();
        node = next;
    }
}

// ---------------------------------------------------------------------------
// Segment cache
// ---------------------------------------------------------------------------
// Returning each 32MiB segment to the OS makes segment churn syscall-bound
// (mimalloc avoids this with arenas). Cache standard segments on a global
// free stack: physical pages are released with MADV_FREE on push, but the
// mapping stays alive so reuse needs no syscall at all.

const CACHE_MAX: usize = 64;

struct CachedRegion {
    next: *mut CachedRegion,
    mem: OsMem,
    expire: i64, // purge physical pages if still cached past this time
    purged: bool,
}

struct Cache {
    lock: core::sync::atomic::AtomicBool,
    first: core::cell::Cell<*mut CachedRegion>,
    count: AtomicUsize,
}
// SAFETY: `first` is only accessed under `lock`.
unsafe impl Sync for Cache {}

static CACHE: Cache = Cache {
    lock: core::sync::atomic::AtomicBool::new(false),
    first: core::cell::Cell::new(ptr::null_mut()),
    count: AtomicUsize::new(0),
};

/// Earliest expiry among cached regions (0 = nothing pending). Checked
/// lock-free on the allocation path; only when it passes do we take the
/// cache lock (the previous unconditional lock+walk serialized all threads
/// in `malloc_generic`).
static PURGE_EXPIRE: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

fn cache_locked<R>(f: impl FnOnce() -> R) -> R {
    crate::sync::spin_locked(&CACHE.lock, f)
}

/// Try to cache a standard segment region; false if the cache is full or the
/// region is non-standard.
pub fn cache_push(mem: OsMem, standard: bool, committed_size: usize) -> bool {
    if !standard || CACHE.count.load(Ordering::Relaxed) >= CACHE_MAX {
        return false;
    }
    // Ensure the whole mapping is committed (cached regions are handed out
    // as fully committed): lazily committed segments may have uncommitted
    // ranges and decommit-mode purges may have dropped some.
    if !mem.is_committed || crate::options::purge_decommits() != 0 {
        // SAFETY: we own the dead region.
        unsafe { commit(mem.base, mem.size) };
        // commit() counted the whole range; the caller's already-committed
        // bytes never left the stat.
        crate::stats::STATS.committed.decrease(committed_size);
    }
    // SAFETY: dead region, fully committed above.
    let node = unsafe { tail_node::<CachedRegion>(&mem) };
    let expire = crate::options::clock_now() + 10 * crate::options::purge_delay().max(1);
    // Track the earliest pending expiry (monotonic clock, so a simple
    // "set if sooner or unset" race is benign).
    let cur = PURGE_EXPIRE.load(Ordering::Relaxed);
    if cur == 0 || expire < cur {
        PURGE_EXPIRE.store(expire, Ordering::Relaxed);
    }
    cache_locked(|| {
        // SAFETY: dead region, first page committed, owned by the cache.
        unsafe {
            node.write(CachedRegion {
                next: CACHE.first.get(),
                mem,
                expire,
                purged: false,
            });
        }
        CACHE.first.set(node);
    });
    CACHE.count.fetch_add(1, Ordering::Relaxed);
    true
}

/// Pop a cached standard segment region (already committed, NOT zeroed).
pub fn cache_pop() -> Option<(*mut u8, OsMem)> {
    if CACHE.count.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let mem = cache_locked(|| {
        // SAFETY: list nodes are owned by the cache while linked.
        unsafe {
            let node = CACHE.first.get().as_ref()?;
            CACHE.first.set(node.next);
            Some(node.mem)
        }
    })?;
    CACHE.count.fetch_sub(1, Ordering::Relaxed);
    let mut mem = mem;
    mem.is_zero = false;
    mem.is_committed = true;
    Some((mem.base, mem))
}

/// Purge physical pages of cached regions that sat unused past their
/// expiration (keeps RSS bounded without per-push madvise cost).
fn cache_purge_expired() {
    // Lock-free fast path: nothing pending or not yet due.
    let expire = PURGE_EXPIRE.load(Ordering::Relaxed);
    if expire == 0 || CACHE.count.load(Ordering::Relaxed) == 0 {
        return;
    }
    let now = crate::options::clock_now();
    if now < expire {
        return;
    }
    // Collect candidates under the lock, purge outside it (madvise can be
    // slow and must not extend the critical section). At most 8 regions per
    // pass; the rest keep their purge bit and go out on a later drain.
    let mut to_purge: [(*mut u8, usize); 8] = [(ptr::null_mut(), 0); 8];
    let mut n = 0;
    cache_locked(|| {
        let tail_span = page_size(); // keep the tail node page intact
        let mut next_expire: i64 = 0;
        let mut p = CACHE.first.get();
        // SAFETY: nodes are owned by the cache while linked and we hold the lock.
        while let Some(node) = unsafe { p.as_mut() } {
            if !node.purged && node.expire <= now && node.mem.size > 2 * tail_span && n < 8 {
                node.purged = true;
                to_purge[n] = (node.mem.base, node.mem.size - tail_span);
                n += 1;
            } else if !node.purged && (next_expire == 0 || node.expire < next_expire) {
                next_expire = node.expire;
            }
            p = node.next;
        }
        PURGE_EXPIRE.store(next_expire, Ordering::Relaxed);
    });
    for &(p, len) in &to_purge[..n] {
        if !p.is_null() {
            // SAFETY: purged ranges belong to cached (dead) regions; a
            // racing pop simply refaults the pages.
            unsafe { reset(p, len) };
        }
    }
}

/// Flush the segment cache back to the OS (used by `mi_collect(force)`).
pub fn cache_collect() {
    while let Some((_, mem)) = cache_pop() {
        mem.free();
    }
}

// ---------------------------------------------------------------------------
// Huge OS pages
// ---------------------------------------------------------------------------

/// Allocate `pages` 1GiB huge OS pages (`_mi_os_alloc_huge_os_pages`).
/// Supported on Linux via `MAP_HUGETLB`; unsupported elsewhere (returns
/// `None`, like mimalloc's graceful fallback).
#[cfg(all(target_os = "linux", not(miri)))]
pub fn alloc_huge_os_pages(pages: usize) -> Option<(*mut u8, OsMem)> {
    let size = pages << 30;
    const MAP_HUGE_1GB: libc::c_int = 30 << 26; // MAP_HUGE_SHIFT
    // SAFETY: anonymous huge-page mapping.
    let p = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_HUGETLB | MAP_HUGE_1GB,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return None;
    }
    let p: *mut u8 = p.cast();
    p.expose_provenance();
    Some((
        p,
        OsMem {
            base: p,
            size,
            align: 1 << 30,
            is_committed: true,
            is_zero: true,
        },
    ))
}

#[cfg(not(all(target_os = "linux", not(miri))))]
pub fn alloc_huge_os_pages(_pages: usize) -> Option<(*mut u8, OsMem)> {
    None // no huge OS page support on this platform
}

/// Make a range inaccessible (`_mi_os_protect`); no-op under Miri.
///
/// # Safety
/// `p..p+size` must lie within an owned, committed mapping with no live
/// references.
pub unsafe fn protect(p: *mut u8, size: usize) {
    #[cfg(not(miri))]
    // SAFETY: per caller contract.
    unsafe {
        libc::mprotect(p.cast(), size, libc::PROT_NONE);
    }
    #[cfg(miri)]
    let _ = (p, size);
}

/// Restore read/write access (`_mi_os_unprotect`); no-op under Miri.
///
/// # Safety
/// `p..p+size` must lie within an owned mapping.
pub unsafe fn unprotect(p: *mut u8, size: usize) {
    #[cfg(not(miri))]
    // SAFETY: per caller contract.
    unsafe {
        libc::mprotect(p.cast(), size, libc::PROT_READ | libc::PROT_WRITE);
    }
    #[cfg(miri)]
    let _ = (p, size);
}

// ---------------------------------------------------------------------------
// fork() handling
// ---------------------------------------------------------------------------
// fork() snapshots memory mid-flight: if another thread holds one of the
// allocator's spinlocks at fork, the child would inherit a locked lock (and
// a possibly half-edited list) forever. `prepare` acquires every global
// lock so the snapshot is consistent; `parent`/`child` release them. Other
// threads do not exist in the child, so their heaps/segments simply remain
// abandoned-or-orphaned there (same policy as mimalloc).

/// All global spinlocks, in a fixed acquisition order.
fn fork_locks() -> [&'static AtomicBool; 2] {
    [&CACHE.lock, crate::init::td_cache_lock()]
}

pub(crate) extern "C" fn fork_prepare() {
    crate::segment::abandoned::lock_for_fork();
    for l in fork_locks() {
        crate::sync::spin_acquire(l);
    }
    crate::arena::lock_for_fork();
}

pub(crate) extern "C" fn fork_release() {
    crate::arena::unlock_for_fork();
    for l in fork_locks() {
        crate::sync::spin_release(l);
    }
    crate::segment::abandoned::unlock_for_fork();
}

use core::sync::atomic::AtomicBool;
