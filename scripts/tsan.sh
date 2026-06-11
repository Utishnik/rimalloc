#!/bin/sh
# ThreadSanitizer battery over the REAL allocator. loom exhaustively checks
# the extracted cross-thread protocol (src/protocol.rs); this watches the
# whole allocator — segment reclaim, arena bitmaps, caches, page lifecycle —
# under real threads. Design claim under test: cross-thread access is
# restricted to atomics and constant-while-shared fields.
#
# Requires nightly with rust-src (TSan must instrument std too, hence
# -Zbuild-std + explicit --target):
#   rustup +nightly component add rust-src
set -e
cd "$(dirname "$0")/.."

TARGET=$(rustc +nightly -vV | sed -n 's/^host: //p')
export RUSTFLAGS="-Zsanitizer=thread"
# TSan exits with code 66 on any report, failing the gate via `set -e`.

echo "== TSan: test suite (default features) =="
cargo +nightly test -p rimalloc --tests -Zbuild-std --target "$TARGET"

echo "== TSan: test suite (secure + debug-fill + stats) =="
cargo +nightly test -p rimalloc --tests -Zbuild-std --target "$TARGET" \
  --features "secure debug-fill stats"

echo "== TSan: stress example (ported test-stress.c, 8 threads) =="
cargo +nightly run -p rimalloc --example stress -Zbuild-std \
  --target "$TARGET" -- 8 25 10

echo "TSAN BATTERY PASSED"
