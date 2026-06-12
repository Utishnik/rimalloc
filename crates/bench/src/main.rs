//! Allocator micro-benchmarks, runnable against rimalloc, C mimalloc, or the
//! system allocator (pick with `--features alloc-*`).

#[cfg(feature = "alloc-rimalloc")]
#[global_allocator]
static ALLOC: rimalloc::Rimalloc = rimalloc::Rimalloc;

#[cfg(feature = "alloc-mimalloc")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "alloc-stub")]
#[global_allocator]
static ALLOC: stub::Stub = stub::Stub;

/// A do-nothing per-thread-freelist allocator: the theoretical floor for the
/// hot path. Alloc pops a same-size-class block from a thread-local free list
/// (bump-allocating a fresh 2 MiB chunk on miss); free pushes back. No
/// cross-thread handling, no segments, no purging, no atomics on the hot
/// path. NOT a real allocator (cross-thread frees leak; never returns memory)
/// — exists only to measure how much of each benchmark is irreducible.
#[cfg(feature = "alloc-stub")]
mod stub {
    use std::alloc::{GlobalAlloc, Layout};
    use std::cell::UnsafeCell;

    const CLASSES: usize = 40; // size classes: round up to next of a small set
    const CHUNK: usize = 2 * 1024 * 1024;

    struct Tls {
        free: [*mut u8; CLASSES],
        bump: *mut u8,
        bump_end: *mut u8,
    }

    thread_local! {
        static TLS: UnsafeCell<Tls> = const {
            UnsafeCell::new(Tls {
                free: [std::ptr::null_mut(); CLASSES],
                bump: std::ptr::null_mut(),
                bump_end: std::ptr::null_mut(),
            })
        };
    }

    #[inline(always)]
    fn class_of(size: usize) -> (usize, usize) {
        // class index and the rounded block size (8-byte granular, geometric).
        let s = size.max(8);
        let bits = (usize::BITS - (s - 1).leading_zeros()) as usize;
        let idx = bits.min(CLASSES - 1);
        (idx, 1usize << bits.max(3))
    }

    pub struct Stub;
    // SAFETY: returns suitably-aligned blocks (power-of-two sized >= align);
    // a benchmark-only floor, not a production allocator.
    unsafe impl GlobalAlloc for Stub {
        #[inline(always)]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let (idx, bsize) = class_of(layout.size().max(layout.align()));
            TLS.with(|t| {
                // SAFETY: thread-local, no reentrancy.
                let t = unsafe { &mut *t.get() };
                let head = t.free[idx];
                if !head.is_null() {
                    // SAFETY: head is a live freed block; first word is the link.
                    t.free[idx] = unsafe { *head.cast::<*mut u8>() };
                    return head;
                }
                if t.bump_end.addr() - t.bump.addr() < bsize {
                    let chunk = CHUNK.max(bsize);
                    // SAFETY: standard mmap.
                    let p = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            chunk,
                            libc::PROT_READ | libc::PROT_WRITE,
                            libc::MAP_PRIVATE | libc::MAP_ANON,
                            -1,
                            0,
                        )
                    };
                    t.bump = p.cast();
                    t.bump_end = t.bump.wrapping_add(chunk);
                }
                let p = t.bump;
                t.bump = t.bump.wrapping_add(bsize);
                p
            })
        }
        #[inline(always)]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            let (idx, _) = class_of(layout.size().max(layout.align()));
            TLS.with(|t| {
                // SAFETY: thread-local, no reentrancy.
                let t = unsafe { &mut *t.get() };
                // SAFETY: ptr is a live block; store the old head as its link.
                unsafe { *ptr.cast::<*mut u8>() = t.free[idx] };
                t.free[idx] = ptr;
            });
        }
    }
}

use std::time::Instant;

fn pick(r: &mut usize) -> usize {
    let mut x = *r;
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    *r = x;
    x
}

/// Tight same-size alloc/free pairs (fast-path latency).
fn bench_pairs() -> f64 {
    let start = Instant::now();
    for size in [8usize, 16, 32, 64, 128, 256, 512, 1024] {
        for _ in 0..2_000_000 {
            let v = vec![0u8; size];
            std::hint::black_box(&v);
        }
    }
    start.elapsed().as_secs_f64()
}

/// Random mixed sizes with a large working set (cache + bin behavior).
fn bench_mixed() -> f64 {
    let start = Instant::now();
    let mut r = 42;
    let mut slots: Vec<Vec<u8>> = (0..100_000).map(|_| Vec::new()).collect();
    for _ in 0..5_000_000 {
        let i = pick(&mut r) % slots.len();
        let size = 1 << (pick(&mut r) % 13); // 1B..4KiB
        slots[i] = vec![pick(&mut r) as u8; size];
    }
    std::hint::black_box(&slots);
    start.elapsed().as_secs_f64()
}

/// Producer/consumer: blocks freed on a different thread than allocated
/// (cross-thread free path).
fn bench_xthread() -> f64 {
    let start = Instant::now();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1024);
    let producers: Vec<_> = (0..4)
        .map(|t| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut r = t + 1;
                for _ in 0..500_000 {
                    let size = 1 << (pick(&mut r) % 10);
                    tx.send(vec![1u8; size]).unwrap();
                }
            })
        })
        .collect();
    drop(tx);
    while let Ok(v) = rx.recv() {
        std::hint::black_box(&v);
    }
    for p in producers {
        p.join().unwrap();
    }
    start.elapsed().as_secs_f64()
}

/// Many short-lived threads (thread init/teardown + abandonment).
fn bench_threads() -> f64 {
    let start = Instant::now();
    for _ in 0..200 {
        let hs: Vec<_> = (0..8)
            .map(|t| {
                std::thread::spawn(move || {
                    let mut r = t + 1;
                    let mut keep = Vec::new();
                    for i in 0..10_000 {
                        let size = 1 << (pick(&mut r) % 12);
                        let v = vec![2u8; size];
                        if i % 16 == 0 {
                            keep.push(v);
                        }
                    }
                    keep.len()
                })
            })
            .collect();
        for h in hs {
            std::hint::black_box(h.join().unwrap());
        }
    }
    start.elapsed().as_secs_f64()
}

/// Real workload: build a large JSON document, then parse + tree-walk +
/// re-serialize it repeatedly across threads (serde_json is intensely
/// allocation-heavy: Maps, Vecs, Strings of every size class).
fn bench_json() -> f64 {
    let mut doc = String::from("[");
    let mut r = 7usize;
    for i in 0..20_000 {
        if i > 0 {
            doc.push(',');
        }
        doc.push_str(&format!(
            r#"{{"id":{i},"name":"user-{i}-{}","tags":["a","bb","ccc","dddd"],"nested":{{"x":{},"y":[1,2,3,4,5,6,7,8],"s":"{}"}}}}"#,
            pick(&mut r) % 1000,
            pick(&mut r) % 100_000,
            "padding-".repeat(pick(&mut r) % 8 + 1),
        ));
    }
    doc.push(']');

    let start = Instant::now();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let doc = doc.clone();
            std::thread::spawn(move || {
                let mut total = 0usize;
                for _ in 0..12 {
                    let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
                    // walk the tree (touch every allocation)
                    fn walk(v: &serde_json::Value) -> usize {
                        match v {
                            serde_json::Value::Array(a) => a.iter().map(walk).sum::<usize>() + 1,
                            serde_json::Value::Object(o) => {
                                o.values().map(walk).sum::<usize>() + o.len()
                            }
                            serde_json::Value::String(s) => s.len(),
                            _ => 1,
                        }
                    }
                    total += walk(&v);
                    total += serde_json::to_string(&v).unwrap().len();
                }
                total
            })
        })
        .collect();
    for h in handles {
        std::hint::black_box(h.join().unwrap());
    }
    start.elapsed().as_secs_f64()
}

/// Real workload: regex compilation + scanning (compilation allocates DFA
/// state machines; scanning allocates capture vectors).
fn bench_regex() -> f64 {
    let mut hay = String::new();
    let mut r = 99usize;
    for i in 0..200_000 {
        hay.push_str(&format!(
            "line {i} value={} key=k{} ",
            pick(&mut r) % 9973,
            i % 511
        ));
    }
    let start = Instant::now();
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let hay = hay.clone();
            std::thread::spawn(move || {
                let mut n = 0usize;
                for i in 0..6 {
                    let re = regex::Regex::new(&format!(r"value=(\d+{}) key=(k\d+)", (t + i) % 10))
                        .unwrap();
                    n += re.captures_iter(&hay).count();
                }
                n
            })
        })
        .collect();
    for h in handles {
        std::hint::black_box(h.join().unwrap());
    }
    start.elapsed().as_secs_f64()
}

/// Port of mimalloc-bench `larson` (server workload): T threads each own
/// an array of slots; every op frees a random slot and allocates a random
/// size in its place. Periodically each thread hands its whole slot array
/// to a fresh thread (cross-thread frees + thread churn).
fn bench_larson() -> f64 {
    const THREADS: usize = 8;
    const SLOTS: usize = 1000;
    const ROUNDS: usize = 4; // thread generations
    const OPS: usize = 1_000_000; // per thread per generation
    const MIN: usize = 8;
    const MAX: usize = 1000;

    let start = Instant::now();
    let mut handles: Vec<std::thread::JoinHandle<Vec<(usize, usize)>>> = (0..THREADS)
        .map(|t| {
            std::thread::spawn(move || {
                let mut r = t * 17 + 1;
                (0..SLOTS)
                    .map(|_| {
                        let size = MIN + pick(&mut r) % (MAX - MIN);
                        (rimalloc_alloc(size).addr(), size)
                    })
                    .collect()
            })
        })
        .collect();

    for round in 0..ROUNDS {
        handles = handles
            .into_iter()
            .enumerate()
            .map(|(t, h)| {
                let slots = h.join().unwrap();
                std::thread::spawn(move || {
                    let mut slots = slots;
                    let mut r = t * 31 + round * 7 + 1;
                    for _ in 0..OPS {
                        let i = pick(&mut r) % SLOTS;
                        rimalloc_free_sized(
                            std::ptr::with_exposed_provenance_mut(slots[i].0),
                            slots[i].1,
                        );
                        let size = MIN + pick(&mut r) % (MAX - MIN);
                        let p = std::hint::black_box(rimalloc_alloc(size));
                        // SAFETY: `p` is a live allocation of at least `size` bytes.
                        unsafe { p.write_bytes(0xb5, size.min(128)) };
                        slots[i] = (p.addr(), size);
                    }
                    slots
                })
            })
            .collect();
    }
    for h in handles {
        for (a, size) in h.join().unwrap() {
            rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(a), size);
        }
    }
    start.elapsed().as_secs_f64()
}

/// Port of mimalloc-bench `xmalloc-test`: producer threads allocate,
/// consumer threads free everything they receive (pure cross-thread-free
/// throughput).
fn bench_xmalloc_test() -> f64 {
    const PAIRS: usize = 4;
    const PER: usize = 2_000_000;
    let start = Instant::now();
    let mut handles = Vec::new();
    for t in 0..PAIRS {
        let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, usize)>(4096);
        handles.push(std::thread::spawn(move || {
            let mut r = t + 1;
            for _ in 0..PER {
                let size = (pick(&mut r) % 256) + 8;
                let p = std::hint::black_box(rimalloc_alloc(size));
                // SAFETY: `p` is a live allocation of at least `size >= 8` bytes.
                unsafe { p.write_bytes(1, 8) };
                tx.send((p.addr(), size)).unwrap();
            }
        }));
        handles.push(std::thread::spawn(move || {
            while let Ok((a, size)) = rx.recv() {
                rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(a), size);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_secs_f64()
}

/// Channel-free producer/consumer ping-pong: isolates the cross-thread
/// free pipeline (mt push + owner collect) from channel overhead.
fn bench_xpipe() -> f64 {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const PAIRS: usize = 4;
    const BATCH: usize = 50_000;
    const ROUNDS: usize = 40;
    let start = Instant::now();
    let mut handles = Vec::new();
    for t in 0..PAIRS {
        let slots: &'static Vec<AtomicUsize> = Box::leak(Box::new(
            (0..BATCH).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>(),
        ));
        let turn: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
        handles.push(std::thread::spawn(move || {
            let mut r = t + 1;
            for round in 0..ROUNDS {
                for slot in slots.iter() {
                    let size = (pick(&mut r) % 256) + 8;
                    let p = std::hint::black_box(rimalloc_alloc(size));
                    // SAFETY: `p` is a live allocation of at least 8 bytes.
                    unsafe { p.write_bytes(1, 8) };
                    slot.store(p.addr() << 9 | size, Ordering::Relaxed);
                }
                turn.store(2 * round + 1, Ordering::Release);
                while turn.load(Ordering::Acquire) != 2 * round + 2 {
                    std::hint::spin_loop();
                }
            }
        }));
        handles.push(std::thread::spawn(move || {
            for round in 0..ROUNDS {
                while turn.load(Ordering::Acquire) != 2 * round + 1 {
                    std::hint::spin_loop();
                }
                for slot in slots.iter() {
                    let v = slot.load(Ordering::Relaxed);
                    rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(v >> 9), v & 0x1ff);
                }
                turn.store(2 * round + 2, Ordering::Release);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_secs_f64()
}

/// xmalloc with a lock-free SPSC ring instead of `sync_channel`: same
/// fine-grained concurrent alloc/cross-free interplay, no parking.
fn bench_xring() -> f64 {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const PAIRS: usize = 4;
    const PER: usize = 2_000_000;
    const RING: usize = 4096;
    struct Ring {
        slots: Vec<AtomicUsize>,
        head: AtomicUsize,
        tail: AtomicUsize,
    }
    let start = Instant::now();
    let mut handles = Vec::new();
    for t in 0..PAIRS {
        let ring: &'static Ring = Box::leak(Box::new(Ring {
            slots: (0..RING).map(|_| AtomicUsize::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }));
        handles.push(std::thread::spawn(move || {
            let mut r = t + 1;
            for i in 0..PER {
                let size = (pick(&mut r) % 256) + 8;
                let p = std::hint::black_box(rimalloc_alloc(size));
                // SAFETY: `p` is a live allocation of at least 8 bytes.
                unsafe { p.write_bytes(1, 8) };
                while i - ring.tail.load(Ordering::Acquire) >= RING {
                    std::hint::spin_loop();
                }
                ring.slots[i % RING].store(p.addr() << 9 | size, Ordering::Relaxed);
                ring.head.store(i + 1, Ordering::Release);
            }
        }));
        handles.push(std::thread::spawn(move || {
            for i in 0..PER {
                while ring.head.load(Ordering::Acquire) <= i {
                    std::hint::spin_loop();
                }
                let v = ring.slots[i % RING].load(Ordering::Relaxed);
                rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(v >> 9), v & 0x1ff);
                ring.tail.store(i + 1, Ordering::Release);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_secs_f64()
}

/// Port of Berger's `cache-scratch` (passive/active false sharing): the
/// main thread allocates one small object per worker; each worker frees
/// it, allocates its own, and writes it in a tight loop. Allocators that
/// hand different threads the same cache line collapse here.
fn bench_cache_scratch() -> f64 {
    const THREADS: usize = 8;
    const OBJ: usize = 8;
    const ITERS: usize = 100_000_000;
    let handoff: Vec<usize> = (0..THREADS).map(|_| rimalloc_alloc(OBJ).addr()).collect();
    let start = Instant::now();
    let handles: Vec<_> = handoff
        .into_iter()
        .map(|a| {
            std::thread::spawn(move || {
                rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(a), OBJ);
                let p = std::hint::black_box(rimalloc_alloc(OBJ));
                for i in 0..ITERS {
                    // volatile so the cache-line traffic actually happens
                    // SAFETY: `p` is a live allocation of `OBJ == 8` bytes.
                    unsafe { std::ptr::write_volatile(p.cast::<u64>(), i as u64) };
                }
                rimalloc_free_sized(p, OBJ);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_secs_f64()
}

/// Route through the selected global allocator via Vec-free raw calls.
fn rimalloc_alloc(size: usize) -> *mut u8 {
    // SAFETY: the layout is valid and non-zero sized.
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size, 8).unwrap()) }
}
fn rimalloc_free_sized(p: *mut u8, size: usize) {
    if !p.is_null() {
        // SAFETY: `p` was allocated by `rimalloc_alloc` with the same layout.
        unsafe { std::alloc::dealloc(p, std::alloc::Layout::from_size_align(size, 8).unwrap()) };
    }
}

/// Fragmentation probe: build a large mixed working set, free a random
/// half (Swiss-cheese pattern), then allocate a different size class into
/// the holes. Reports peak RSS and retained RSS per live byte — external
/// fragmentation shows up as RSS that stays high after the frees.
fn bench_frag() -> f64 {
    fn rss_mb() -> f64 {
        let mut info = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage fills the struct; assume_init only after success.
        unsafe {
            libc::getrusage(libc::RUSAGE_SELF, info.as_mut_ptr());
            info.assume_init()
        }
        .ru_maxrss as f64
            / (1 << 20) as f64
    }
    let mut r = 5usize;
    // phase 1: ~400MB of mixed small/medium blocks
    let mut blocks: Vec<(usize, usize)> = (0..400_000)
        .map(|_| {
            let size = 16 << (pick(&mut r) % 7); // 16B..1KiB
            let size = size + pick(&mut r) % size;
            (rimalloc_alloc(size).addr(), size)
        })
        .collect();
    let live_full: usize = blocks.iter().map(|b| b.1).sum();
    let peak = rss_mb();
    // phase 2: free a random half
    let mut i = 0;
    blocks.retain(|(a, size)| {
        i += 1;
        if i % 2 == pick(&mut r) % 2 {
            rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(*a), *size);
            false
        } else {
            true
        }
    });
    rimalloc_collect_if_available();
    let live_half: usize = blocks.iter().map(|b| b.1).sum();
    // phase 3: different size class into the fragmented heap
    let refill: Vec<(usize, usize)> = (0..100_000)
        .map(|_| {
            let size = 2048 + pick(&mut r) % 6144;
            (rimalloc_alloc(size).addr(), size)
        })
        .collect();
    let live_refill: usize = refill.iter().map(|b| b.1).sum();
    let end = rss_mb();
    println!(
        "  frag: peak {peak:.0}MB over {:.0}MB live; after half-free + {:.0}MB refill: {end:.0}MB RSS over {:.0}MB live (overhead x{:.2})",
        live_full as f64 / (1 << 20) as f64,
        live_refill as f64 / (1 << 20) as f64,
        (live_half + live_refill) as f64 / (1 << 20) as f64,
        end / ((live_half + live_refill) as f64 / (1 << 20) as f64),
    );
    for (a, size) in blocks.into_iter().chain(refill) {
        rimalloc_free_sized(std::ptr::with_exposed_provenance_mut(a), size);
    }
    0.0
}

#[cfg(feature = "alloc-rimalloc")]
fn rimalloc_collect_if_available() {
    rimalloc::collect(true);
}
#[cfg(not(feature = "alloc-rimalloc"))]
fn rimalloc_collect_if_available() {}

#[cfg(feature = "alloc-rimalloc")]
fn syscall_counts() -> [usize; 4] {
    rimalloc::os_syscall_counts()
}
#[cfg(not(feature = "alloc-rimalloc"))]
fn syscall_counts() -> [usize; 4] {
    [0; 4]
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let name = if cfg!(feature = "alloc-rimalloc") {
        "rimalloc"
    } else if cfg!(feature = "alloc-mimalloc") {
        "mimalloc"
    } else {
        "system"
    };
    println!("allocator: {name}");
    let reps: usize = std::env::var("BENCH_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let show_syscalls = std::env::var("BENCH_SYSCALLS").is_ok();
    let run = |bench: &str, f: fn() -> f64| {
        if which == "all" || which == bench {
            let before = syscall_counts();
            let t = (0..reps).map(|_| f()).fold(f64::INFINITY, f64::min);
            if show_syscalls {
                let after = syscall_counts();
                let d: Vec<i64> = before
                    .iter()
                    .zip(after.iter())
                    .map(|(b, a)| *a as i64 - *b as i64)
                    .collect();
                println!("{bench:>10}: {t:.3}s  syscalls[mmap,munmap,madvise,mprotect]={d:?}");
            } else {
                println!("{bench:>10}: {t:.3}s");
            }
        }
    };
    run("pairs", bench_pairs);
    run("mixed", bench_mixed);
    run("xthread", bench_xthread);
    run("threads", bench_threads);
    run("json", bench_json);
    run("regex", bench_regex);
    run("larson", bench_larson);
    run("xmalloc", bench_xmalloc_test);
    run("xpipe", bench_xpipe);
    run("xring", bench_xring);
    run("scratch", bench_cache_scratch);
    run("frag", bench_frag);
}
