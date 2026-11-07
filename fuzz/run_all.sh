#!/usr/bin/env bash
# Run every fuzz target for a fixed wall-clock budget, a few at a time, and keep the logs.
#
#   fuzz/run_all.sh [seconds-per-target] [parallel-jobs]
#
# Each target runs with libFuzzer's -timeout (an input slower than this is reported as a hang and
# saved under fuzz/artifacts/<target>/timeout-*) and -rss_limit_mb (an input that pushes the process
# over this is saved as oom-*), so "no panic, no infinite loop, bounded memory" are all enforced by
# the same run. Crashes land in fuzz/artifacts/<target>/crash-*. Logs go to fuzz/logs/<target>.log.
set -u
cd "$(dirname "$0")"
SECONDS_PER_TARGET="${1:-600}"
JOBS="${2:-3}"
mkdir -p logs artifacts corpus
# The live corpus (git-ignored) starts from the committed seeds.
for d in seeds/*/; do t=$(basename "$d"); mkdir -p "corpus/$t"; cp -n "$d"* "corpus/$t/" 2>/dev/null; done
TARGETS=$(grep -E '^name = "' Cargo.toml | sed -E 's/name = "(.*)"/\1/' | grep -v freedombooks-fuzz | grep -v "^$")

run_one() {
  local t="${1:-}"; [ -z "$t" ] && return 0
  local timeout=10
  local rss=2048
  local max_len=4096
  case "$t" in
    app_form|backup_archive|oauth_callback) timeout=30 ;;
    simplefin_account_set|attachments_inspect) max_len=65536 ;;
    bankfeeds_text) timeout=5; max_len=2048 ;;
  esac
  mkdir -p "corpus/$t" "artifacts/$t"
  echo "[$(date -u +%H:%M:%S)] start $t for ${SECONDS_PER_TARGET}s"
  cargo fuzz run "$t" "corpus/$t" -- \
    -max_total_time="$SECONDS_PER_TARGET" -timeout="$timeout" -rss_limit_mb="$rss" -max_len="$max_len" \
    -print_final_stats=1 -artifact_prefix="artifacts/$t/" >"logs/$t.log" 2>&1
  echo "[$(date -u +%H:%M:%S)] done  $t (exit $?)"
}

export -f run_one
export SECONDS_PER_TARGET
printf '%s\n' $TARGETS | xargs -P "$JOBS" -I{} bash -c 'run_one {}'
echo "all targets finished"
