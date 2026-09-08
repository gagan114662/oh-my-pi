#!/usr/bin/env bash
# Marks a soak phase boundary from the WORKFLOW: appends a timestamped record
# and updates the current phase name the sampler copies into every row.
#   usage: phase.sh <out-dir> <phase-name> [key=value ...]
set -euo pipefail
OUT="$1"; NAME="$2"; shift 2
EXTRA=""
for kv in "$@"; do k="${kv%%=*}"; v="${kv#*=}"; EXTRA="$EXTRA,\"$k\":\"$v\""; done
printf '{"ts":%s,"phase":"%s"%s}\n' "$(date +%s.%N)" "$NAME" "$EXTRA" >> "$OUT/phases.jsonl"
printf '%s' "$NAME" > "$OUT/phase.txt"
echo "phase: $NAME $* at $(date -u +%FT%TZ)"
