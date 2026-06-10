/- Emit the proven model's input/output vectors so the Rust test suite can
   check the live implementation against the verified model exactly.
   Regenerate with `lake exe genvectors` only when the SPEC changes. -/
import RimallocVerify.Bins

open Rimalloc

def main : IO Unit := do
  let h ← IO.FS.Handle.mk "vectors/bins.csv" IO.FS.Mode.write
  -- w,binFromWsize(w) over the full domain incl. one past the bins' range
  for w in [0:8194] do
    h.putStrLn s!"{w},{binFromWsize w}"
  let h2 ← IO.FS.Handle.mk "vectors/bin_sizes.csv" IO.FS.Mode.write
  -- bin,binWsize(bin) for every queue index
  for b in [0:75] do
    h2.putStrLn s!"{b},{binWsize b}"
