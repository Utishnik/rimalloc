#!/bin/sh
# Run every verification gate for rimalloc. Fails on the first broken gate.
set -e
cd "$(dirname "$0")/.."

echo "== build (release) =="
cargo build --release

echo "== clippy (deny warnings) =="
cargo clippy --release -- -D warnings

echo "== fmt check =="
cargo fmt --check

echo "== native tests (unit + ported mimalloc suite) =="
cargo test

echo "== stress (ported test-stress.c) =="
cargo run --release --example stress -- 16 25 10

echo "== Miri (aliasing-model verification) =="
cargo miri test -p rimalloc

echo "== loom model check (delayed-free protocol) =="
LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" cargo test -p rimalloc --test loom_protocol --release

echo "== Lean proofs =="
if command -v lake >/dev/null 2>&1 || [ -x "$HOME/.elan/bin/lake" ]; then
  PATH="$HOME/.elan/bin:$PATH" sh -c 'cd verify && lake build'
else
  echo "lake not found; skipping Lean (install via elan)"
fi

echo "ALL GATES PASSED"
