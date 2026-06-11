#!/bin/sh
# Long-running fuzz soak with persistent corpus. Each target gets half the
# time budget. Usage: scripts/soak.sh [total_seconds]   (default: 4 hours)
set -e
cd "$(dirname "$0")/../crates/rimalloc"
TOTAL=${1:-14400}
PER=$((TOTAL / 3))
for target in alloc_ops heap_api mt_ops; do
  echo "== soaking $target for ${PER}s (corpus persists in fuzz/corpus/$target) =="
  cargo +nightly fuzz run "$target" -- \
    -max_total_time="$PER" -max_len=4096 -rss_limit_mb=6144 \
    -print_final_stats=1
done
echo "== fork-hang watchdog (100 runs, 60s budget each) =="
cargo +stable test --features "secure debug-fill stats" --test fork --no-run
FORKBIN=$(cargo +stable test --features "secure debug-fill stats" --test fork --no-run --message-format=json 2>/dev/null | python3 -c "import json,sys
for l in sys.stdin:
    try: j=json.loads(l)
    except: continue
    if j.get('executable'): print(j['executable'])" | head -1)
for i in $(seq 1 100); do
  "$FORKBIN" > /dev/null 2>&1 &
  TPID=$!
  for w in $(seq 1 60); do sleep 1; kill -0 $TPID 2>/dev/null || break; done
  if kill -0 $TPID 2>/dev/null; then
    echo "FORK HANG on iteration $i (pid $TPID) — sampling"
    sample $TPID 2 -file "fork-hang-$i.sample" 2>/dev/null
    CH=$(pgrep -P $TPID | head -1)
    [ -n "$CH" ] && sample $CH 2 -file "fork-hang-$i-child.sample" 2>/dev/null
    kill -9 $TPID $CH 2>/dev/null
    exit 1
  fi
done

echo "SOAK COMPLETE: no crashes"
