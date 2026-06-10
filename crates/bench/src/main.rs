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
}
