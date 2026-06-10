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
- [x] Stress (32 threads × 50 iters): 0.66s vs C mimalloc v2.3.2's 0.62s
      (within ~7%; syscalls at parity after segment/thread caching)

Microbenchmarks (`crates/bench`, Apple Silicon, vs the `mimalloc` crate
bundling C mimalloc **v3.3.2** and the macOS system allocator —
**rimalloc wins all four**):

| bench | rimalloc | mimalloc v3.3.2 | system |
|---|---|---|---|
| pairs (same-size alloc/free) | **0.108s** | 0.128s | 0.320s |
| mixed (random sizes, 100k slots) | **0.336s** | 0.414s | 0.945s |
| xthread (producer/consumer) | **0.178s** | 0.189s | 0.255s |
| threads (thread churn) | **0.058s** | 0.067s | 0.120s |

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
- Secure mode (free-list encoding, guard pages), debug fill patterns.
- Extend the Lean verification beyond the current proofs.

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
