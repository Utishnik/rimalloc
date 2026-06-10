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
| `options.c` | `options.rs` | env-configurable options |

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

### Parity gaps / roadmap

- **Arenas** (`arena.c`): mimalloc's 1GiB reserved arenas with bitmap
  allocation; replaces the simpler segment cache, enables
  `mi_reserve_os_memory`, NUMA affinity, delayed purge batching.
- Statistics (`stats.c`), options table completeness, `mi_register_*` hooks.
- `mi_heap_new_in_arena`, heap tags beyond the basics.
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
- **`FreeList.lean`** — the page free-list counting invariant
  (`used + |free| + |local_free| = capacity ≤ reserved`,
  `|thread_free| ≤ used`) is preserved by alloc, local free, remote free,
  thread-collect, collect, and extend — so `used == 0` soundly means
  "no live blocks" in `page_retire`/`page_free`.

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
