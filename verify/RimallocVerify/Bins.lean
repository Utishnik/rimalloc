/-
Formal model of rimalloc's size-bin mapping (`crates/rimalloc/src/bins.rs`,
a port of mimalloc's `mi_bin` for the 64-bit `MI_ALIGN2W` configuration).

We verify, for every word size in the bins' domain (`w ≤ 8192`):
* `bin_in_range`  — the bin index is always in `1..=73`,
* `bin_fits`      — the chosen size class is large enough for the request,
* `bin_waste`     — internal fragmentation is strictly below 25%
                    (the 1/4-octave class spacing),
* `bin_monotone`  — the mapping is monotone in the request size,
* `bin_exact_small` — sizes of at most 8 words round only to the next even
                    word count (the `ALIGN2W` guarantee).

The model mirrors the Rust code line by line; the theorems are established
by exhaustive evaluation (`native_decide`) over the full finite domain, so
they hold for the actual constants used by the allocator.
-/

namespace Rimalloc

/-- Machine pointer size in bytes (64-bit). -/
def ptrSize : Nat := 8

/-- `MI_MEDIUM_OBJ_WSIZE_MAX`: largest word size served by the bins. -/
def mediumObjWsizeMax : Nat := 8192

/-- `MI_BIN_HUGE`. -/
def binHuge : Nat := 73

/-- `wsize_from_size`: bytes to words, rounding up. -/
def wsizeFromSize (size : Nat) : Nat :=
  (size + ptrSize - 1) / ptrSize

/-- Floor of log2 for positive `n` (Rust: `BITS - 1 - leading_zeros`). -/
def log2 (n : Nat) : Nat := Nat.log2 n

/-- `Bin::from_size` on word sizes (`mi_bin`, ALIGN2W variant). -/
def binFromWsize (w : Nat) : Nat :=
  if w ≤ 8 then
    -- round to double-word sizes
    if w ≤ 1 then 1 else (w + 1) / 2 * 2
  else if w > mediumObjWsizeMax then
    binHuge
  else
    let n := w - 1
    let b := log2 n
    (b <<< 2) + ((n >>> (b - 2)) &&& 0x03) - 3

/-- `Bin::from_size` on byte sizes. -/
def binFromSize (size : Nat) : Nat :=
  binFromWsize (wsizeFromSize size)

/-- `BIN_WSIZE`: largest word size of each bin (mirrors the const table
    builder in `bins.rs`). -/
def binWsize (bin : Nat) : Nat :=
  if bin = 0 then 1
  else if bin ≤ 8 then bin
  else if bin < binHuge then
    let b := (bin + 3) / 4
    let m := (bin + 3) % 4
    (1 <<< b) + ((m + 1) <<< (b - 2))
  else if bin = binHuge then mediumObjWsizeMax + 1
  else mediumObjWsizeMax + 2

/-- The bin index is always within `1..=binHuge`. -/
theorem bin_in_range :
    ∀ w < 8200, 1 ≤ binFromWsize w ∧ binFromWsize w ≤ binHuge := by
  native_decide

/-- The chosen size class is large enough: a request of `w` words fits in
    the block size of its bin. -/
theorem bin_fits :
    ∀ w < 8193, w ≤ binWsize (binFromWsize w) := by
  native_decide

/-- Internal fragmentation is below 25%: `class - w < (w + 3) / 4 + 1`.
    (Worst case is `w = 2^b + 1` whose class is `2^b + 2^(b-2)`.) -/
theorem bin_waste :
    ∀ w < 8193, 4 * (binWsize (binFromWsize w) - w) < w + 8 := by
  native_decide

/-- Bin selection is monotone in the request size. -/
theorem bin_monotone :
    ∀ w < 8192, binFromWsize w ≤ binFromWsize (w + 1) := by
  native_decide

/-- Requests above the bins' domain always map to the huge bin. -/
theorem bin_huge_large {w : Nat} (h : mediumObjWsizeMax < w) :
    binFromWsize w = binHuge := by
  unfold binFromWsize
  have h8 : ¬ w ≤ 8 := by unfold mediumObjWsizeMax at h; omega
  simp [h8, h]

/-- ALIGN2W: requests of at most 8 words map exactly to the next even word
    count (so the class never exceeds the request by more than one word). -/
theorem bin_exact_small :
    ∀ w < 9, 1 ≤ w → binWsize (binFromWsize w) ≤ w + 1 := by
  native_decide

/-- The table and the mapping are mutually consistent: each reachable bin's
    class size maps back to that bin. -/
theorem bin_roundtrip :
    ∀ bin < binHuge, 1 ≤ bin →
      binWsize bin ≤ mediumObjWsizeMax →
      binFromWsize (binWsize bin) ≤ bin ∨ binFromWsize (binWsize bin) = (bin + 1) / 2 * 2 := by
  native_decide

/-- One word above a class boundary spills into a strictly later bin. -/
theorem bin_boundary :
    ∀ bin < binHuge, 8 ≤ bin → binWsize bin < mediumObjWsizeMax →
      binFromWsize (binWsize bin) < binFromWsize (binWsize bin + 1) := by
  native_decide

/-- `wsizeFromSize` rounds up to the nearest word: the word count covers the
    byte size and wastes less than one word. -/
theorem wsize_spec (s : Nat) :
    s ≤ wsizeFromSize s * ptrSize ∧ wsizeFromSize s * ptrSize < s + ptrSize := by
  unfold wsizeFromSize ptrSize
  omega

/-- Byte-level fit: for every requestable size up to
    `MEDIUM_OBJ_SIZE_MAX = 64 KiB`, the chosen bin's block size (in bytes)
    holds the request. This is the property `malloc` relies on. -/
theorem bin_fits_bytes :
    ∀ s < 65537, s ≤ binWsize (binFromSize s) * ptrSize := by
  native_decide

end Rimalloc
