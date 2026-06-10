//! Fuzzes the first-class heap API: heap creation, allocation across
//! multiple live heaps, `heap_delete` (blocks survive, absorbed by the
//! backing heap) and `heap_destroy` (blocks die with the heap),
//! interleaved with default-heap traffic and heap walks. The shadow model
//! tracks which heap owns each block and verifies fill patterns before
//! every free; deleted heaps' blocks must remain intact, destroyed heaps'
//! blocks are forgotten without freeing.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rimalloc::Heap;

struct Live {
    ptr: *mut u8,
    size: usize,
    pat: u8,
    heap: usize, // index into heaps, usize::MAX = default heap
}

fn fill(p: *mut u8, n: usize, pat: u8) {
    unsafe { core::ptr::write_bytes(p, pat, n.min(4096)) }
}
fn check(l: &Live) {
    for i in 0..l.size.min(4096) {
        assert_eq!(unsafe { *l.ptr.add(i) }, l.pat, "corruption in heap {} block", l.heap);
    }
}

fuzz_target!(|data: &[u8]| {
    let mut heaps: Vec<Option<&'static Heap>> = Vec::new();
    let mut live: Vec<Live> = Vec::new();
    let mut pat = 0x40u8;
    let mut it = data.iter().copied();
    let mut n2 = |it: &mut dyn Iterator<Item = u8>| {
        u16::from_le_bytes([it.next().unwrap_or(5), it.next().unwrap_or(1)]) as usize
    };

    while let Some(op) = it.next() {
        match op % 6 {
            0 => {
                // create a heap (bounded)
                if heaps.iter().flatten().count() < 4 {
                    heaps.push(rimalloc::heap_new());
                }
            }
            1 => {
                // allocate on a random live heap (or the default heap)
                let size = (n2(&mut it) & 0x3FFF) + 1;
                let pick = n2(&mut it) % (heaps.len() + 1);
                let (hidx, p) = match heaps.get(pick).and_then(|h| h.as_ref()) {
                    Some(h) => (pick, rimalloc::heap_malloc(h, size)),
                    None => (usize::MAX, rimalloc::malloc(size)),
                };
                if !p.is_null() {
                    pat = pat.wrapping_add(1).max(1);
                    fill(p, size, pat);
                    live.push(Live { ptr: p, size, pat, heap: hidx });
                }
            }
            2 => {
                // free a random block (any heap; cross-heap free is legal)
                if !live.is_empty() {
                    let l = live.swap_remove(n2(&mut it) % live.len());
                    check(&l);
                    rimalloc::free(l.ptr);
                }
            }
            3 => {
                // delete a heap: its blocks transfer to the backing heap
                // and MUST remain intact and freeable
                if let Some(pos) = heaps.iter().position(|h| h.is_some()) {
                    let h = heaps[pos].take().unwrap();
                    rimalloc::heap_delete(h);
                    for l in live.iter_mut().filter(|l| l.heap == pos) {
                        check(l);
                        l.heap = usize::MAX;
                    }
                }
            }
            4 => {
                // destroy a heap: its blocks die without individual frees
                if let Some(pos) = heaps.iter().position(|h| h.is_some()) {
                    let h = heaps[pos].take().unwrap();
                    live.retain(|l| {
                        if l.heap == pos {
                            check(l);
                            false // memory dies with the heap
                        } else {
                            true
                        }
                    });
                    rimalloc::heap_destroy(h);
                }
            }
            _ => {
                // walk the default heap (exercises visit + free-map paths)
                let mut blocks = 0usize;
                rimalloc::heap_visit_blocks(rimalloc::heap_get_default(), true, &mut |_, b| {
                    if b.is_some() {
                        blocks += 1;
                    }
                    blocks < 10_000
                });
            }
        }
        if live.len() > 256 {
            let l = live.swap_remove(0);
            check(&l);
            rimalloc::free(l.ptr);
        }
    }
    // teardown: free survivors, delete remaining heaps
    for l in live.drain(..) {
        check(&l);
        rimalloc::free(l.ptr);
    }
    for h in heaps.into_iter().flatten() {
        rimalloc::heap_delete(h);
    }
});
