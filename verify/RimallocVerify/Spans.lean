/-
Model of a segment's slice spans (`segment.rs` / `segment.c`): the slices
array is a *partition* of the segment into used and free spans. Freeing a
span coalesces it with free neighbors (`span_free_coalesce`); allocation
may split a free span (`slice_split`).

Proven, for the coalescing normalization `norm`:
* `norm_total`     — the partition is preserved (no slices gained or lost),
* `norm_wf`        — spans stay non-empty,
* `norm_coalesced` — the result is maximal: no two adjacent free spans
                     remain, which is what makes the O(1) neighbor-merge in
                     `span_free_coalesce` complete and lets the span-queue
                     best-fit search assume maximal spans.
And for the two mutations:
* `split_total`    — `slice_split` preserves the partition,
* `flip_total`     — marking a span used↔free preserves the partition.

The same invariants are asserted on live segments by the debug validator
`segment_assert_valid` in `segment.rs` (port of `mi_segment_is_valid`).
-/

namespace Rimalloc

inductive Span where
  | used : Nat → Span
  | free : Nat → Span
deriving DecidableEq, Repr

namespace Span

def count : Span → Nat
  | .used n => n
  | .free n => n

def isFree : Span → Bool
  | .used _ => false
  | .free _ => true

end Span

/-- Total slice count of a span list (the partition measure). -/
def total (l : List Span) : Nat :=
  (l.map Span.count).sum

/-- Well-formed: every span covers at least one slice. -/
def Wf (l : List Span) : Prop :=
  ∀ s ∈ l, 0 < s.count

/-- Maximality: no two adjacent free spans. -/
def Coalesced : List Span → Prop
  | [] => True
  | [_] => True
  | s :: t :: rest => ¬(s.isFree = true ∧ t.isFree = true) ∧ Coalesced (t :: rest)

/-- Coalescing normalization: single pass merging adjacent free spans. -/
def norm : List Span → List Span
  | [] => []
  | s :: rest =>
    match s, norm rest with
    | .free a, .free b :: tl => .free (a + b) :: tl
    | .free a, [] => [.free a]
    | .free a, .used b :: tl => .free a :: .used b :: tl
    | .used a, tl => .used a :: tl

/-- `norm` preserves the partition total. -/
theorem norm_total (l : List Span) : total (norm l) = total l := by
  induction l with
  | nil => rfl
  | cons s rest ih =>
    cases s with
    | used a => simp [norm, total, Span.count] at ih ⊢; omega
    | free a =>
      cases htl : norm rest with
      | nil => simp [norm, htl, total, Span.count] at ih ⊢; omega
      | cons t tl =>
        cases t with
        | used b => simp [norm, htl, total, Span.count] at ih ⊢; omega
        | free b => simp [norm, htl, total, Span.count] at ih ⊢; omega

/-- `norm` keeps spans non-empty. -/
theorem norm_wf (l : List Span) (h : Wf l) : Wf (norm l) := by
  induction l with
  | nil => exact h
  | cons s rest ih =>
    have hs : 0 < s.count := h s (by simp)
    have hrest : Wf rest := fun x hx => h x (by simp [hx])
    have ihw : Wf (norm rest) := ih hrest
    intro x hx
    cases s with
    | used a =>
      simp only [norm, List.mem_cons] at hx
      rcases hx with rfl | hx
      · exact hs
      · exact ihw x hx
    | free a =>
      cases htl : norm rest with
      | nil =>
        simp only [norm, htl, List.mem_cons] at hx
        rcases hx with rfl | hx
        · exact hs
        · simp at hx
      | cons t tl =>
        cases t with
        | used b =>
          simp only [norm, htl, List.mem_cons] at hx
          rcases hx with rfl | rfl | hx
          · exact hs
          · exact ihw (.used b) (by simp [htl])
          · exact ihw x (by simp [htl, hx])
        | free b =>
          have hb : 0 < (Span.free b).count := ihw (.free b) (by simp [htl])
          simp only [norm, htl, List.mem_cons] at hx
          rcases hx with rfl | hx
          · simp [Span.count] at hb ⊢; omega
          · exact ihw x (by simp [htl, hx])

/-- `norm` produces a maximal (fully coalesced) partition. -/
theorem norm_coalesced (l : List Span) : Coalesced (norm l) := by
  induction l with
  | nil => trivial
  | cons s rest ih =>
    cases s with
    | used a =>
      simp only [norm]
      cases htl : norm rest with
      | nil => trivial
      | cons t tl =>
        rw [htl] at ih
        exact ⟨by simp [Span.isFree], ih⟩
    | free a =>
      cases htl : norm rest with
      | nil => simp [norm, htl]; trivial
      | cons t tl =>
        rw [htl] at ih
        cases t with
        | used b =>
          simp only [norm, htl]
          exact ⟨by simp [Span.isFree], ih⟩
        | free b =>
          simp only [norm, htl]
          -- merged head `free (a+b)` sits before `tl`, whose head (if any)
          -- is not free since `free b :: tl` was coalesced
          cases tl with
          | nil => trivial
          | cons u tl' =>
            obtain ⟨hu, htl'⟩ := ih
            refine ⟨?_, htl'⟩
            intro ⟨_, hfu⟩
            exact hu ⟨rfl, hfu⟩

/-- `slice_split`: splitting a used span into a used head and free tail
    preserves the partition. -/
theorem split_total (pre post : List Span) (k c : Nat) (h : k ≤ c) :
    total (pre ++ Span.used k :: Span.free (c - k) :: post)
      = total (pre ++ Span.used c :: post) := by
  simp [total, Span.count]
  omega

/-- Marking a span used↔free preserves the partition. -/
theorem flip_total (pre post : List Span) (c : Nat) :
    total (pre ++ Span.used c :: post) = total (pre ++ Span.free c :: post) := by
  simp [total, Span.count]

end Rimalloc
