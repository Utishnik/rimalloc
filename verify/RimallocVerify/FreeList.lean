/-
Abstract model of a mimalloc page's three free lists and the counting
invariant from `types.h` (and `page.rs`):

    used + |free| + |local_free| = capacity ≤ reserved
    |thread_free| ≤ used

We model the page as counters (list lengths) plus the owning operations:
allocation pops `free`; local free pushes `local_free`; a remote (cross
thread) free pushes `thread_free`; `thread_collect` migrates `thread_free`
into `local_free` (decrementing `used`); `collect` moves `local_free` to
`free` when `free` is empty; `extend` adds fresh blocks. Each operation is
proven to preserve the invariant, so by induction every reachable page
state satisfies it — this is the property that makes `used == 0` a sound
"page is free" test in `page_retire`/`page_free`.
-/

namespace Rimalloc

structure Page where
  reserved   : Nat
  capacity   : Nat
  used       : Nat
  free       : Nat  -- |free|
  localFree  : Nat  -- |local_free|
  threadFree : Nat  -- |thread_free|

/-- The page invariant (`types.h` comments, checked by `_mi_page_is_valid`). -/
structure Inv (p : Page) : Prop where
  count  : p.used + p.free + p.localFree = p.capacity
  cap    : p.capacity ≤ p.reserved
  remote : p.threadFree ≤ p.used

/-- `_mi_page_malloc`: pop a block from `free`. -/
def alloc (p : Page) : Page :=
  if p.free = 0 then p
  else { p with free := p.free - 1, used := p.used + 1 }

/-- `mi_free_block_local`: the owner frees a live block. A live block is
    one counted in `used` but not pending in `thread_free`. -/
def freeLocal (p : Page) : Page :=
  if p.threadFree < p.used then
    { p with used := p.used - 1, localFree := p.localFree + 1 }
  else p

/-- `mi_free_block_mt`: another thread frees a live block (pushes on the
    atomic `thread_free` list; `used` still counts it). -/
def freeRemote (p : Page) : Page :=
  if p.threadFree < p.used then { p with threadFree := p.threadFree + 1 } else p

/-- `_mi_page_thread_free_collect`: take the whole `thread_free` list and
    append it to `local_free`, adjusting `used`. -/
def threadCollect (p : Page) : Page :=
  { p with
    used := p.used - p.threadFree,
    localFree := p.localFree + p.threadFree,
    threadFree := 0 }

/-- `_mi_page_free_collect` (second half): move `local_free` to `free`
    when `free` is exhausted. -/
def collect (p : Page) : Page :=
  if p.free = 0 then { p with free := p.localFree, localFree := 0 } else p

/-- `mi_page_extend_free`: commit `n` more blocks into `free`. -/
def extend (p : Page) (n : Nat) : Page :=
  if p.capacity + n ≤ p.reserved then
    { p with capacity := p.capacity + n, free := p.free + n }
  else p

theorem inv_alloc {p : Page} (h : Inv p) : Inv (alloc p) := by
  unfold alloc
  split
  · exact h
  · obtain ⟨c, cap, r⟩ := h
    constructor <;> simp_all <;> omega

theorem inv_freeLocal {p : Page} (h : Inv p) : Inv (freeLocal p) := by
  unfold freeLocal
  split
  · obtain ⟨c, cap, r⟩ := h
    constructor <;> simp_all <;> omega
  · exact h

theorem inv_freeRemote {p : Page} (h : Inv p) : Inv (freeRemote p) := by
  unfold freeRemote
  split
  · obtain ⟨c, cap, r⟩ := h
    constructor <;> simp_all <;> omega
  · exact h

theorem inv_threadCollect {p : Page} (h : Inv p) : Inv (threadCollect p) := by
  obtain ⟨c, cap, r⟩ := h
  unfold threadCollect
  constructor <;> simp_all <;> omega

theorem inv_collect {p : Page} (h : Inv p) : Inv (collect p) := by
  unfold collect
  split
  · obtain ⟨c, cap, r⟩ := h
    constructor <;> simp_all <;> omega
  · exact h

theorem inv_extend {p : Page} (h : Inv p) (n : Nat) : Inv (extend p n) := by
  unfold extend
  split
  · obtain ⟨c, cap, r⟩ := h
    constructor <;> simp_all <;> omega
  · exact h

/-- A fresh page (`page_init` before `extend`) satisfies the invariant. -/
def fresh (reserved : Nat) : Page :=
  { reserved, capacity := 0, used := 0, free := 0, localFree := 0, threadFree := 0 }

theorem inv_fresh (reserved : Nat) : Inv (fresh reserved) := by
  constructor <;> simp [fresh]

/-- Soundness of the "all free" test used by `page_retire`/`_mi_page_free`:
    once `used = 0` (after a collect), every committed block really is on
    `free ++ local_free`, i.e. no live blocks remain. -/
theorem all_free_sound {p : Page} (h : Inv p) (hu : p.used = 0) :
    p.free + p.localFree = p.capacity ∧ p.threadFree = 0 := by
  obtain ⟨c, _, r⟩ := h
  omega

end Rimalloc
