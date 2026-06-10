//! Allocator micro-benchmarks, runnable against rimalloc, C mimalloc, or the
//! system allocator (pick with `--features alloc-*`).

#[cfg(feature = "alloc-rimalloc")]
#[global_allocator]
static ALLOC: rimalloc::Rimalloc = rimalloc::Rimalloc;

#[cfg(feature = "alloc-mimalloc")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
    let run = |bench: &str, f: fn() -> f64| {
        if which == "all" || which == bench {
            println!("{bench:>10}: {:.3}s", f());
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
}
