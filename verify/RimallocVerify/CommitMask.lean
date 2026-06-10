/-
Model of `CommitMask::create` (`segment.rs`): the loop that sets
`bitcount` bits starting at `bitidx` across an array of 8×64-bit fields.
We prove the loop equals the specification mask `((1 <<< count) - 1) <<< idx`
for every valid `(idx, count)` pair — 512×513 cases, checked exhaustively
by compiled evaluation.
-/

namespace Rimalloc

def maskBits : Nat := 512
def fieldBits : Nat := 64

/-- The Rust loop, modeled on a single natural number (fields concatenated
    little-endian, faithful to the per-field arithmetic). -/
def createLoop (bitidx bitcount : Nat) : Nat :=
  if bitcount = maskBits then (1 <<< maskBits) - 1
  else go (bitidx / fieldBits) (bitidx % fieldBits) bitcount 0
where
  go (field ofs left acc : Nat) : Nat :=
    if left = 0 then acc
    else
      let avail := fieldBits - ofs
      if avail = 0 then acc -- unreachable: `ofs < fieldBits` throughout
      else
        let count := min left avail
        let m := if count = fieldBits then (1 <<< fieldBits) - 1
                 else ((1 <<< count) - 1) <<< ofs
        go (field + 1) 0 (left - count) (acc ||| (m <<< (field * fieldBits)))
  termination_by left
  decreasing_by omega

/-- Specification: a contiguous run of `count` ones starting at `idx`. -/
def createSpec (bitidx bitcount : Nat) : Nat :=
  ((1 <<< bitcount) - 1) <<< bitidx

/-- The loop meets the spec on every in-range pair. -/
theorem create_correct :
    ∀ idx ≤ maskBits, ∀ count ≤ maskBits - idx,
      createLoop idx count = createSpec idx count := by
  native_decide

/-- Bit-level reading of the spec (so `create_correct` transfers to
    `testBit`-style reasoning about committed slices). -/
theorem spec_testBit (idx count i : Nat) :
    (createSpec idx count).testBit i ↔ (idx ≤ i ∧ i < idx + count) := by
  unfold createSpec
  rw [Nat.testBit_shiftLeft]
  have h1 : (1 <<< count - 1).testBit (i - idx) = decide (i - idx < count) := by
    rw [Nat.shiftLeft_eq, Nat.one_mul, Nat.testBit_two_pow_sub_one]
  rw [h1]
  simp only [Bool.and_eq_true, decide_eq_true_eq]
  omega

end Rimalloc
