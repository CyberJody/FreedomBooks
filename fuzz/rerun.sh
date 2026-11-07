#!/usr/bin/env bash
# Second pass for the targets whose harness assertions were relaxed after their first finding
# (see FUZZ-AUDIT-REPORT.md): 10 minutes each, three at a time.
set -u
cd "$(dirname "$0")"
run_one() {
  local t="${1:-}"; [ -z "$t" ] && return 0
  local timeout=10; local max_len=4096
  case "$t" in app_form) timeout=30 ;; attachments_inspect) max_len=65536 ;; bankfeeds_text) timeout=5; max_len=2048 ;; esac
  echo "[$(date -u +%H:%M:%S)] start $t for 600s"
  cargo fuzz run "$t" "corpus/$t" -- -max_total_time=600 -timeout="$timeout" -rss_limit_mb=2048 -max_len="$max_len" -print_final_stats=1 -artifact_prefix="artifacts/$t/" >"logs/$t.log" 2>&1
  echo "[$(date -u +%H:%M:%S)] done  $t (exit $?)"
}
export -f run_one
printf '%s\n' py_int_uuid simplefin_scalars app_form attachments_inspect bankfeeds_text | xargs -P 3 -I{} bash -c 'run_one {}'
echo "rerun finished"
