#!/bin/sh
# Long-running fuzz soak with persistent corpus. Each target gets half the
# time budget. Usage: scripts/soak.sh [total_seconds]   (default: 4 hours)
set -e
cd "$(dirname "$0")/../crates/rimalloc"
TOTAL=${1:-14400}
PER=$((TOTAL / 2))
for target in alloc_ops heap_api; do
  echo "== soaking $target for ${PER}s (corpus persists in fuzz/corpus/$target) =="
  cargo +nightly fuzz run "$target" -- \
    -max_total_time="$PER" -max_len=4096 -rss_limit_mb=6144 \
    -print_final_stats=1
done
echo "SOAK COMPLETE: no crashes"
