# rimalloc

A pure-Rust port of [mimalloc](https://github.com/microsoft/mimalloc) v2.3.2 —
a compact, high-performance general-purpose allocator with per-thread heaps,
sharded free lists, and lock-free cross-thread frees.

```rust
use rimalloc::Rimalloc;

#[global_allocator]
static ALLOC: Rimalloc = Rimalloc;
```

or the `mi_*`-style API directly:

```rust
let p = rimalloc::malloc(42);
assert!(rimalloc::usable_size(p) >= 42);
rimalloc::free(p);
```

## Architecture (mirrors mimalloc v2)

| mimalloc | rimalloc | role |
|---|---|---|
| `types.h` | `constants.rs`, `page.rs`, `segment.rs`, `heap.rs` | core data structures |
| `page-queue.c` | `bins.rs`, `heap.rs` | 73 size bins + page queues |
| `segment.c` | `segment.rs` | 32MiB segments, 64KiB slices, span queues, abandonment |
| `page.c` | `page_ops.rs` | page init/extend/retire, generic malloc |
| `alloc.c` | `alloc.rs`, `api.rs` | malloc fast path, public API |
| `free.c` | `free.rs` | free fast path, cross-thread frees |
| `heap.c` | `heap_ops.rs` | collect, delete, destroy, absorb |
| `init.c` | `init.rs` | TLS heaps, thread lifecycle |
| `os.c`, `prim/unix` | `os.rs` | mmap/commit/purge + segment cache |
| `options.c` | `options.rs` | full get/set option API, env-configurable |
| `arena.c` | `arena.rs` | 1GiB+ reservations, bitmap block claiming, lazy purge |
| `stats.c` | `stats.rs` | process stats, `stats_print` (`stats` feature for per-alloc) |

Key invariants (from `types.h`):
- `used - |thread_free|` = live blocks in a page
- `used + |free| + |local_free| = capacity <= reserved`
- free spans partition each segment's slice array; span heads carry
  `slice_count`, interior slices carry back-offsets for interior-pointer lookup

### Rust-specific design

- **No `&mut` anywhere in the core**: all metadata fields are `Cell`/atomics
  accessed through shared references, since pages/segments are reachable from
  other threads (cross-thread free lists). This is both sound and free.
- **Provenance-correct**: segment mappings expose their provenance; the
  `ptr → segment` address mask reconstructs pointers via
  `with_exposed_provenance`. Neighbor-slice arithmetic goes through the
  segment's `slices` array. The test suite passes under **Miri** (Stacked
  Borrows) using a `cfg(miri)` OS shim backed by `std::alloc`.
- **Deferred unmaps**: segments are never returned to the OS while references
  into them may sit on the call stack; dead regions queue on an intrusive
  pending list drained at API boundaries.
- **Segment & thread-metadata caches**: dead segments and thread data are
  recycled on lock-free stacks, making segment/thread churn syscall-free
  (mimalloc uses arenas for this; see gaps below). Cached segments release
  their physical pages with a delayed `MADV_FREE` if they sit unused.

## Status

- [x] Full malloc/zalloc/calloc/realloc/aligned/posix API + heap API
- [x] `GlobalAlloc` implementation
- [x] Per-thread heaps, thread exit abandonment, segment reclamation
- [x] Heap walking (`heap_visit_blocks` with a closure-based visitor)
- [x] Arena layer: geometric 1GiB+ reservations, lock-free single-block
      claims, batched lazy purge — segment churn is fully syscall-free
- [x] Statistics (`stats_print`; per-allocation accounting behind the
      `stats` feature, like `MI_STAT`)
- [x] Options API (`options::get/set/enable/disable`, `MIMALLOC_*` env)
- [x] Secure mode (`secure` feature): encoded free lists with per-page and
      per-heap random keys, corrupted-list and double-free detection
      (~5% overhead, mirroring `MI_SECURE=3/4`)
- [x] Ported test suite: `test-api.c`, `test-api-fill.c`, `test-stress.c`
- [x] Miri-clean on the ported suite
- [x] Stress (`test-stress.c` port, instruction-parity harness on both
      sides): rimalloc is ~5% *ahead* of C mimalloc v2.3.2 on user time
      (interleaved mins of 3: 23.0s vs 24.3s at `32 50 200`, 5.70s vs
      6.04s at `32 50 50`). Earlier reports of an ~18% deficit traced to
      a stale `test-stress.o` in the C reference build left over from a
      transfer-ablation experiment — the C binary was running with the
      bench's 25% cross-thread exchange compiled out. With both sides
      instrumented on identical pristine sources the hand-off machinery
      is at op-count parity per run (mt pushes 19.0M vs 19.2M; full-page
      delayed-free pings 1.56M vs 1.50M; `page_unfull`/`page_to_full`
      churn 1.40M on both), and rimalloc pointer-chases *fewer*
      remote-dirty lines during adoption (12.0M vs 17.7M blocks walked)
      because the count rides in the thread-free word instead of being
      recounted by walking the chain.

Microbenchmarks (`crates/bench`, min of 3 interleaved runs, Apple M3 Max
(16 cores, macOS 15, one background core busy during measurement), vs the
`mimalloc` crate bundling C mimalloc **v3.3.2** and the macOS system
allocator — **rimalloc wins every allocator-sensitive bench**):

| bench | rimalloc | mimalloc v3.3.2 | system |
|---|---|---|---|
| pairs (same-size alloc/free) | **0.132s** | 0.163s | 0.321s |
| mixed (random sizes, 100k slots) | **0.337s** | 0.408s | 0.779s |
| xthread (producer/consumer) | **0.191s** | 0.214s | 0.217s |
| threads (thread churn) | **0.058s** | 0.077s | 0.122s |
| json (serde_json parse+walk+serialize, 8 threads) | **0.266s** | 0.290s | 0.566s |
| larson (server: random size mix, cross-thread frees) | **0.050s** | 0.089s | 0.406s |
| xmalloc (producer/consumer free pressure) | **0.086s** | 0.154s | 0.202s |
| xpipe (xmalloc over `sync_channel`) | **0.048s** | 0.056s | 0.160s |
| xring (xmalloc over lock-free SPSC ring) | **0.122s** | 0.166s | 0.235s |
| scratch (cache-scratch false sharing; insensitive) | 0.036s | 0.036s | 0.036s |
| regex (control: allocator-insensitive) | 0.042s | 0.038s | 0.041s |

larson is at the physical floor of the benchmark: a stub allocator that
only pops/pushes per-thread freelists (no metadata, no size bins, fully
inlined) runs the identical loop in 0.049s in the same windows — the
remaining time is the bench's own memset/RNG and the dependent loads on
its ~500KB working set.

Stable Rust is supported with no fast-path penalty: on Apple arm64 the
default heap lives in a dedicated pthread TSD slot (as C mimalloc's
`MI_TLS_SLOT`), so malloc/free pay no `_tlv_get_addr` call; elsewhere the
opt-in `nightly` feature switches `thread_local!` to raw `#[thread_local]`.
`fork()` is handled via `pthread_atfork` lock ordering and tested under
multithreaded load.

- [x] `reserve_os_memory` / `manage_os_memory` (adopt external memory),
      exclusive arenas, `heap_new_in_arena` / `heap_new_ex` (heap tags),
      NUMA plumbing, huge OS pages API (Linux `MAP_HUGETLB`; graceful
      fallback elsewhere)
- [x] Guard pages under `secure`: a protected page between segment
      metadata and page data plus one at the segment end (`MI_SECURE>=1`)
- [x] `register_deferred_free` / `register_output` / `register_error`
      hooks (heartbeat-driven, recursion-guarded, provenance-correct)
- [x] `is_in_heap_region`

- [x] Debug fill patterns (`debug-fill` feature, `MI_DEBUG` 0xD0/0xDF
      uninit/freed stamping) with the corresponding `test-api-fill.c` tests

### Parity gaps / roadmap

- Per-page guard pages (`MI_SECURE=5`'s most expensive level).
- Page-granular purging inside live segments is ported; tuning it to match
  C's burst peak-RSS exactly remains.
- First-class heaps as a typed Rust API (`Heap` is already exposed).
- Extend the Lean verification beyond the current proofs.

## Fuzzing

`crates/rimalloc/fuzz` holds a libFuzzer target (`alloc_ops`) that decodes
fuzz input into allocator operation sequences — malloc/zalloc/calloc/free/
realloc/aligned/collect across every size class and page kind — against a
shadow model with per-block fill patterns (corruption shows up as pattern
mismatches; the debug validators assert the Lean-proven invariants on
every step, and ASan watches the borders). 200k runs clean.

A second target (`heap_api`) fuzzes the first-class heap API: multi-heap
allocation, `heap_delete` (blocks must survive absorption into the backing
heap), `heap_destroy` (blocks die with the heap), and heap walks.

```
cd crates/rimalloc && cargo +nightly fuzz run alloc_ops -- -rss_limit_mb=6144
scripts/soak.sh 14400   # 4h soak across both targets, persistent corpus
```

## Concurrency model checking (loom)

The cross-thread delayed-free protocol — the one subsystem Lean cannot
reach — lives as a *single generic implementation* in `src/protocol.rs`:
production `free.rs`/`page_ops.rs` call it with `core` atomics on live
pages/heaps, and `tests/loom_protocol.rs` calls the *same functions* with
loom atomics, exploring every interleaving of the real CAS structure,
memory orderings, and FREEING-flag transitions. Checked: no freed block is
ever lost or duplicated between the page list and the heap delayed list,
`used` reaches exactly zero, and FREEING never leaks. Editing the protocol
automatically re-subjects it to the model check — there is no mirror to
drift.

```
LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" \
  cargo test -p rimalloc --test loom_protocol --release
```

## Formal verification (Lean 4)

`verify/` contains a Lean 4 (lake) project with machine-checked proofs over
faithful models of the allocator's core arithmetic:

- **`Bins.lean`** — the size-class mapping (`mi_bin`/`Bin::from_size`):
  bin indices stay in `1..=73`; the chosen class always fits the request;
  internal fragmentation stays below 25%; the mapping is monotone; small
  sizes round only to the next even word (ALIGN2W); table/mapping
  round-trip consistency. (Exhaustive `native_decide` over the full domain.)
- **`CommitMask.lean`** — the commit-mask `create` loop equals the
  closed-form `((1 << count) - 1) << idx` for all 512-bit (idx, count)
  pairs, plus a `testBit` characterization.
- **`Spans.lean`** — segment span coalescing: normalization preserves the
  slice partition, keeps spans non-empty, and yields *maximal* spans (no
  adjacent free spans) — the soundness basis for O(1) neighbor-merging;
  splitting and state flips preserve the partition.
- **`PageStart.lean`** — the page block-area offset: 16-alignment, the
  block-size alignment guarantee for every *reachable* class up to
  `MAX_ALIGN_GUARANTEE` (the proof shows reachability is essential — a
  24-byte class would be broken by the 16-byte round-up, which is exactly
  why ALIGN2W rounds odd word counts away), and that a block always fits.
- **`FreeList.lean`** — the page free-list counting invariant
  (`used + |free| + |local_free| = capacity ≤ reserved`,
  `|thread_free| ≤ used`) is preserved by alloc, local free, remote free,
  thread-collect, collect, and extend — so `used == 0` soundly means
  "no live blocks" in `page_retire`/`page_free`.

The proofs are **bridged to the live implementation**:
`lake exe genvectors` evaluates the proven model over its full domain into
committed vectors, and `tests/lean_conformance.rs` pins the Rust functions
to them exactly — refactor freely; `cargo test` re-verifies that every
theorem still transfers. The infinite-state invariants (free-list counts,
span partition/maximality/back-offsets) are asserted on the *live*
allocator by debug-build validators at every page collect and page free.

```
cd verify && lake build   # all proofs check, no sorries
```

## Testing

```
cargo test                         # unit + ported API/fill suites
cargo run --release --example stress -- 32 50 50
cargo miri test -p rimalloc        # aliasing-model check (std::alloc shim)
sh scripts/check.sh                # every gate: build, clippy -D warnings,
                                   # fmt, tests, stress, Miri, Lean proofs
```

## License

MIT (same as mimalloc). The C sources in `reference/` are vendored for
porting reference only and are not part of the crate.
