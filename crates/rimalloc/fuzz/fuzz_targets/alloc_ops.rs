//! Coverage-guided fuzzing of the allocator API: fuzz input is decoded as a
//! sequence of operations (malloc/zalloc/calloc/free/realloc/aligned/heap)
//! against a shadow model. Every live block carries a fill pattern that is
//! verified before any free/realloc — heap corruption (overlapping blocks,
//! free-list damage) surfaces as a pattern mismatch, and the debug-build
//! validators assert the Lean-proven page/segment invariants on every
//! collect and page free. Sizes are shaped to hit every bin, page kind
//! (small/medium/large/huge), and the aligned-overallocation paths.
#![no_main]

use libfuzzer_sys::fuzz_target;

struct Live {
    ptr: *mut u8,
    size: usize,
    pat: u8,
}

/// Big blocks get head+tail pages stamped (overlap corruption between
/// blocks shows up at boundaries); small blocks are stamped fully.
const DENSE: usize = 8192;

fn fill(p: *mut u8, size: usize, pat: u8) {
    if size <= 2 * DENSE {
        unsafe { core::ptr::write_bytes(p, pat, size) }
    } else {
        unsafe {
            core::ptr::write_bytes(p, pat, DENSE);
            core::ptr::write_bytes(p.add(size - DENSE), pat, DENSE);
        }
    }
}

fn check(l: &Live) {
    let span = |off: usize, len: usize| {
        for i in off..off + len {
            let b = unsafe { *l.ptr.add(i) };
            assert_eq!(
                b, l.pat,
                "heap corruption: byte {i} of {} (ptr {:p}) is {b:#x}, expected {:#x}",
                l.size, l.ptr, l.pat
            );
        }
    };
    if l.size <= 2 * DENSE {
        span(0, l.size);
    } else {
        span(0, DENSE);
        span(l.size - DENSE, DENSE);
    }
}

/// Shape a raw 16-bit size onto interesting allocator regions.
fn shape_size(raw: u16) -> usize {
    let s = raw as usize;
    match s >> 13 {
        0..=4 => (s & 0x3FF) + 1,            // 1..1024: small bins
        5 => ((s & 0x1FFF) << 3) + 1,        // up to 64KiB: medium
        6 => ((s & 0xFFF) << 9) + 1,         // up to 2MiB: large
        _ => ((s & 0x7) << 20) + 1,          // up to 8MiB: huge path
    }
}

const MAX_LIVE_BYTES: usize = 768 << 20; // keep under libFuzzer's RSS limit

fuzz_target!(|data: &[u8]| {
    let mut live: Vec<Live> = Vec::new();
    let mut live_bytes = 0usize;
    let mut pat: u8 = 0xA0;
    let mut it = data.iter().copied();
    let mut next2 =
        |it: &mut dyn Iterator<Item = u8>| u16::from_le_bytes([it.next().unwrap_or(7), it.next().unwrap_or(3)]);

    while let Some(op) = it.next() {
        match op % 8 {
            0 | 1 => {
                // malloc / zalloc
                let size = shape_size(next2(&mut it));
                let p = if op % 8 == 0 { rimalloc::malloc(size) } else { rimalloc::zalloc(size) };
                if p.is_null() {
                    continue; // OOM is allowed, crashing is not
                }
                assert!(rimalloc::usable_size(p) >= size, "usable < requested");
                if op % 8 == 1 {
                    for i in 0..size.min(512) {
                        assert_eq!(unsafe { *p.add(i) }, 0, "zalloc not zero");
                    }
                }
                pat = pat.wrapping_add(1).max(1);
                fill(p, size, pat);
                live_bytes += size;
                live.push(Live { ptr: p, size, pat });
            }
            2 => {
                // free a random live block (pattern-verified)
                if live.is_empty() {
                    continue;
                }
                let i = next2(&mut it) as usize % live.len();
                let l = live.swap_remove(i);
                check(&l);
                live_bytes -= l.size;
                rimalloc::free(l.ptr);
            }
            3 => {
                // realloc a random live block; old contents must survive
                if live.is_empty() {
                    continue;
                }
                let i = next2(&mut it) as usize % live.len();
                check(&live[i]);
                let newsize = shape_size(next2(&mut it));
                let old = live[i].size;
                let q = rimalloc::realloc(live[i].ptr, newsize);
                if q.is_null() {
                    continue; // block unchanged on failure
                }
                for j in 0..old.min(newsize).min(DENSE) {
                    assert_eq!(unsafe { *q.add(j) }, live[i].pat, "realloc lost contents");
                }
                pat = pat.wrapping_add(1).max(1);
                fill(q, newsize, pat);
                live_bytes = live_bytes - old + newsize;
                live[i] = Live { ptr: q, size: newsize, pat };
            }
            4 => {
                // aligned allocation (alignments 8..=4MiB, incl. overalloc path)
                let size = shape_size(next2(&mut it));
                let align = 8usize << (next2(&mut it) % 20);
                let p = rimalloc::malloc_aligned(size, align);
                if p.is_null() {
                    continue;
                }
                assert_eq!(p.addr() % align, 0, "alignment violated");
                assert!(rimalloc::usable_size(p) >= size);
                pat = pat.wrapping_add(1).max(1);
                fill(p, size, pat);
                live_bytes += size;
                live.push(Live { ptr: p, size, pat });
            }
            5 => {
                // calloc with overflow-prone counts
                let count = next2(&mut it) as usize;
                let size = next2(&mut it) as usize;
                let p = rimalloc::calloc(count, size);
                if let Some(total) = count.checked_mul(size) {
                    if !p.is_null() {
                        for j in (0..total.min(DENSE)).step_by(512) {
                            assert_eq!(unsafe { *p.add(j) }, 0, "calloc not zero");
                        }
                        pat = pat.wrapping_add(1).max(1);
                        fill(p, total, pat);
                        live_bytes += total;
                        live.push(Live { ptr: p, size: total, pat });
                    }
                } else {
                    assert!(p.is_null(), "calloc overflow not detected");
                }
            }
            6 => {
                // collect (exercises purge/retire under churn)
                rimalloc::collect(next2(&mut it) % 7 == 0);
            }
            _ => {
                // verify a random live block without freeing
                if !live.is_empty() {
                    let i = next2(&mut it) as usize % live.len();
                    check(&live[i]);
                    assert!(rimalloc::usable_size(live[i].ptr) >= live[i].size);
                }
            }
        }
        while live.len() > 512 || live_bytes > MAX_LIVE_BYTES {
            let l = live.swap_remove(0);
            check(&l);
            live_bytes -= l.size;
            rimalloc::free(l.ptr);
        }
    }
    for l in live.drain(..) {
        check(&l);
        rimalloc::free(l.ptr);
    }
});
