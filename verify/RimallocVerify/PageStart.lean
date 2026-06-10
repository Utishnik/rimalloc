/-
Model of `segment.rs::page_start_offset` (the pure core of
`_mi_segment_page_start_from_slice`): the block area of a page is nudged
off OS-page/cache boundaries by a start offset. The contract the allocator
relies on (asserted in C by `mi_assert_internal` in `segment.c`):

* `pageStart_align16`  — the start stays `MAX_ALIGN_SIZE`(16)-aligned,
* `pageStart_blockAligned` — for block sizes up to
  `MAX_ALIGN_GUARANTEE` (64 KiB) the start is *block-size* aligned —
  this is what `mi_malloc_is_naturally_aligned` and the aligned-alloc
  fast path depend on,
* `pageStart_fits`     — at least one block fits behind the offset.

Subtle point the proof hinges on: rounding the offset up to 16 would
break block-size alignment for arbitrary block sizes (e.g. 24), but every
*reachable* page block size is a bin size, and under ALIGN2W those are
either 8 or multiples of 16 — so the round-up is harmless. The theorems
quantify over exactly the reachable bins and every possible 16-aligned
page-start residue (segment bases and slices are 16-aligned), checked
exhaustively by `native_decide`.
-/
import RimallocVerify.Bins

namespace Rimalloc

/-- `MI_MAX_ALIGN_GUARANTEE` (= `MEDIUM_OBJ_SIZE_MAX`, 64 KiB). -/
def maxAlignGuarantee : Nat := 65536

/-- `page_start_offset` (faithful transcription of the Rust). -/
def startOffset (blockSize pstart psize : Nat) : Nat :=
  let o1 :=
    if 0 < blockSize ∧ blockSize ≤ maxAlignGuarantee then
      let adjust := blockSize - pstart % blockSize
      if adjust < blockSize ∧ blockSize + adjust ≤ psize then adjust else 0
    else 0
  let o2 := o1 +
    (if 8 ≤ blockSize then
      if blockSize ≤ 64 then 3 * blockSize
      else if blockSize ≤ 512 then blockSize
      else 0
    else 0)
  (o2 + 15) / 16 * 16

/-- The page size a block size is served from: small pages (64 KiB) up to
    8 KiB blocks, medium pages (512 KiB) up to 64 KiB blocks. -/
def psizeFor (blockSize : Nat) : Nat :=
  if blockSize ≤ 8192 then 65536 else 524288

/-- Reachable page block sizes ≤ the guarantee: bins 1..=48 (8 B .. 64 KiB). -/
def binBytes (bin : Nat) : Nat := 8 * binWsize bin

/-- The start offset is always 16-aligned (`MI_MAX_ALIGN_SIZE`); since
    segment bases and slice boundaries are 16-aligned, so is the block
    area start. -/
theorem pageStart_align16 :
    ∀ bin < 49, ∀ k < 4096,
      startOffset (binBytes bin) (16 * k) (psizeFor (binBytes bin)) % 16 = 0 := by
  native_decide

/-- A bin is *reachable* if the size mapping can actually select it.
    Under ALIGN2W the odd small bins (3, 5, 7 words) are not: requests
    round to the next even word count. -/
def reachable (bin : Nat) : Bool :=
  binFromWsize (binWsize bin) = bin

/-- Block-size alignment guarantee: for every *reachable* bin size up to
    `MAX_ALIGN_GUARANTEE` and every 16-aligned page start, the block-area
    start is block-size aligned. (`16 * k` for `k < 4096` covers every
    16-aligned residue class mod any `blockSize ≤ 65536`, and the offset
    depends on `pstart` only through `pstart % blockSize`.)

    Reachability is essential: a 24-byte class would be *broken* by the
    16-byte round-up — `native_decide` finds the counterexample if the
    guard is dropped. ALIGN2W rounding odd word counts away is precisely
    what makes the alignment guarantee sound. -/
theorem pageStart_blockAligned :
    ∀ bin < 49, 1 ≤ bin → reachable bin → ∀ k < 4096,
      (16 * k + startOffset (binBytes bin) (16 * k) (psizeFor (binBytes bin)))
        % binBytes bin = 0 := by
  native_decide

/-- At least one block fits behind the offset on its page. -/
theorem pageStart_fits :
    ∀ bin < 49, 1 ≤ bin → ∀ k < 4096,
      startOffset (binBytes bin) (16 * k) (psizeFor (binBytes bin)) + binBytes bin
        ≤ psizeFor (binBytes bin) := by
  native_decide

/-- The offset only depends on `pstart % blockSize` (justifies checking
    residue representatives above). -/
theorem startOffset_residue (bs p q psize : Nat)
    (h : p % bs = q % bs) :
    startOffset bs p psize = startOffset bs q psize := by
  unfold startOffset
  rw [h]

end Rimalloc
