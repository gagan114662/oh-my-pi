#!/usr/bin/env bash
# Sleeps until <minute> minutes after the soak start recorded in <out>/start.epoch.
set -euo pipefail
OUT="$1"; MIN="$2"; START=$(cat "$OUT/start.epoch"); TARGET=$(( START + MIN * 60 ))
NOW=$(date +%s); [ "$NOW" -ge "$TARGET" ] || sleep $(( TARGET - NOW ))
echo "minute $MIN reached at $(date -u +%FT%TZ)"
