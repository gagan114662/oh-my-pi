#!/usr/bin/env bash
# Soak sampler (#105): the WORKFLOW's measurements, taken by the shell, not by omp.
# Appends one CSV row per interval. Every column is a stat/du/ps/pgrep/grep result.
#   usage: sample.sh <out-dir> <session-dir> <data-dir> <omp-binary> <interval-seconds>
set -uo pipefail
OUT="$1"; SESSIONS="$2"; DATA="$3"; OMP="$4"; INTERVAL="${5:-300}"
CSV="$OUT/samples.csv"
[ -f "$CSV" ] || echo "epoch,minute,phase,journal_bytes,blob_bytes,rss_omp_kb,rss_envd_kb,omp_procs,envd_procs,receipts,compactions,stream_entries,assistant_starts,notices" > "$CSV"
START=$(date +%s)
# grep -c prints 0 and exits 1 on no match; never append a fallback to it.
cnt() { local n; n=$(grep -c "$1" "$2" 2>/dev/null); echo "${n:-0}"; }
while true; do
  NOW=$(date +%s); MIN=$(( (NOW - START) / 60 ))
  PHASE=$(cat "$OUT/phase.txt" 2>/dev/null || echo "-")
  J=$(ls -t "$SESSIONS"/*.oms 2>/dev/null | head -1)
  if [ -n "$J" ]; then
    JB=$(stat -c %s "$J" 2>/dev/null || stat -f %z "$J" 2>/dev/null || echo 0)
    RECEIPTS=$(cnt '^event: turn.receipt@1' "$J")
    COMPACT=$(cnt '^event: compaction@1' "$J")
    STREAMS=$(cnt '^event: stream@1' "$J")
    STARTS=$(cnt '^event: msg.assistant.start@1' "$J")
    NOTICES=$(cnt '^event: notice@1' "$J")
  else
    JB=0; RECEIPTS=0; COMPACT=0; STREAMS=0; STARTS=0; NOTICES=0
  fi
  BLOBS=$(du -sb "$DATA" 2>/dev/null | cut -f1 || echo 0)
  RSS_OMP=$(pgrep -f "^$OMP print" | xargs -r ps -o rss= -p 2>/dev/null | awk '{s+=$1} END {print s+0}')
  RSS_ENVD=$(pgrep -f "^$OMP envd" | xargs -r ps -o rss= -p 2>/dev/null | awk '{s+=$1} END {print s+0}')
  NOMP=$(pgrep -f "^$OMP print" 2>/dev/null | wc -l | tr -d " ")
  NENVD=$(pgrep -f "^$OMP envd" 2>/dev/null | wc -l | tr -d " ")
  echo "$NOW,$MIN,$PHASE,$JB,$BLOBS,$RSS_OMP,$RSS_ENVD,${NOMP:-0},${NENVD:-0},$RECEIPTS,$COMPACT,$STREAMS,$STARTS,$NOTICES" >> "$CSV"
  [ -f "$OUT/stop-sampler" ] && exit 0
  sleep "$INTERVAL"
done
