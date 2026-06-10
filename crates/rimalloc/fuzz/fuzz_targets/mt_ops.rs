//! Multithreaded fuzzing: each input drives owner + remote threads through
//! the cross-thread paths the single-threaded targets cannot reach —
//! `free_generic_mt` (page thread-free lists and the heap delayed list),
//! thread-exit abandonment, and reclaim-on-free of abandoned segments.
//! Interleavings are scheduler-dependent (loom owns exhaustiveness; this
//! buys breadth × ASan × the debug validators), but each input is still a
//! deterministic op program. Blocks carry fill patterns verified by
//! whichever thread frees them.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::mpsc;

struct Block(*mut u8, usize, u8);
unsafe impl Send for Block {}

fn fill(p: *mut u8, n: usize, pat: u8) {
    unsafe { core::ptr::write_bytes(p, pat, n.min(4096)) }
}
fn check(b: &Block) {
    for i in 0..b.1.min(4096) {
        assert_eq!(unsafe { *b.0.add(i) }, b.2, "corruption in cross-thread block");
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let n_remote = (data[0] % 3 + 1) as usize; // 1..=3 remote threads
    let ops = &data[1..];

    // Remote threads free whatever they receive (cross-thread free) and
    // also allocate blocks of their own, some of which they send back and
    // some they leak into thread exit (abandonment), for the main thread
    // to free afterwards (reclaim-on-free).
    let mut to_remote: Vec<mpsc::Sender<Block>> = Vec::new();
    let mut handles = Vec::new();
    let (back_tx, back_rx) = mpsc::channel::<Block>();
    for t in 0..n_remote {
        let (tx, rx) = mpsc::channel::<Block>();
        to_remote.push(tx);
        let back = back_tx.clone();
        let seed = data.get(2 + t).copied().unwrap_or(7);
        handles.push(std::thread::spawn(move || {
            let mut k = 0u8;
            while let Ok(b) = rx.recv() {
                check(&b);
                rimalloc::free(b.0);
                // allocate our own block; alternate fates by seed
                let size = ((seed as usize * 131 + k as usize * 977) % 9000) + 1;
                let p = rimalloc::malloc(size);
                if !p.is_null() {
                    let pat = seed ^ k | 1;
                    fill(p, size, pat);
                    if k % 3 == 0 {
                        let _ = back.send(Block(p, size, pat));
                    } else {
                        // freed here; pages this thread owns with the
                        // *received* remote blocks' segments stay live and
                        // are abandoned at exit
                        check(&Block(p, size, pat));
                        rimalloc::free(p);
                    }
                }
                k = k.wrapping_add(1);
            }
        }));
    }
    drop(back_tx);

    let mut local: Vec<Block> = Vec::new();
    let mut pat = 0x10u8;
    for (i, &op) in ops.iter().enumerate() {
        match op % 5 {
            0 | 1 => {
                // alloc and ship to a remote thread (its free is cross-thread)
                let size = ((op as usize) << 5 | i & 31) + 1;
                let p = rimalloc::malloc(size);
                if !p.is_null() {
                    pat = pat.wrapping_add(1) | 1;
                    fill(p, size, pat);
                    let _ = to_remote[i % n_remote].send(Block(p, size, pat));
                }
            }
            2 => {
                let size = (op as usize * 67 % 16384) + 1;
                let p = rimalloc::malloc(size);
                if !p.is_null() {
                    pat = pat.wrapping_add(1) | 1;
                    fill(p, size, pat);
                    local.push(Block(p, size, pat));
                }
            }
            3 => {
                if let Ok(b) = back_rx.try_recv() {
                    check(&b);
                    rimalloc::free(b.0); // remote-allocated, freed here
                }
            }
            _ => {
                if !local.is_empty() {
                    let b = local.swap_remove(op as usize % local.len());
                    check(&b);
                    rimalloc::free(b.0);
                }
            }
        }
    }

    // close channels; remote threads drain, allocate, exit (abandonment)
    drop(to_remote);
    for h in handles {
        h.join().unwrap();
    }
    // free remote-allocated survivors: their segments are abandoned, so
    // these frees exercise attempt_reclaim
    while let Ok(b) = back_rx.recv() {
        check(&b);
        rimalloc::free(b.0);
    }
    for b in local.drain(..) {
        check(&b);
        rimalloc::free(b.0);
    }
    rimalloc::collect(false);
});
