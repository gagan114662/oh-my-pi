#!/usr/bin/env bash
# Plumbing self-test for the soak rig: runs provider, driver, sampler, phase
# markers and report against a STUB omp that only speaks the JSON header and
# appends journal-shaped lines. It proves the rig's wiring, not omp. Real
# evidence comes only from .github/workflows/soak.yml.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
T="$(mktemp -d)"; OUT="$T/out"; SESS="$T/sessions"; DATA="$T/data"; PROJ="$T/proj"
mkdir -p "$OUT" "$SESS" "$DATA" "$PROJ"
cat > "$T/omp" <<'STUB'
#!/usr/bin/env bash
# stub omp: print --mode json ... --session-dir D [--resume ID] prompt
set -u
if [ "${1:-}" = "envd" ]; then sleep 30; exit 0; fi
D=""; ID=""; PORT="${STUB_PORT:?}"
while [ $# -gt 0 ]; do case "$1" in --session-dir) D="$2"; shift;; --resume) ID="$2"; shift;; esac; shift; done
[ -n "$ID" ] || ID="01STUB$(date +%s%N | cut -c1-14)"
J="$D/$ID.oms"
[ -f "$J" ] || printf 'event: journal@1\ndata: {"version":1}\n\n' > "$J"
echo "{\"type\":\"session\",\"version\":1,\"id\":\"$ID\",\"model\":\"mock\"}"
for i in 1 2; do
  code=$(curl -s -o "$D/last.body" -w '%{http_code}' --max-time 400 -H 'content-type: application/json' \
    -d "{\"model\":\"mock\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"$(cat "$J" | tr -d '\n"' | cut -c1-2000) $ID\"}]}" \
    "http://127.0.0.1:$PORT/v1/chat/completions")
  [ "$code" = "200" ] || { printf 'event: notice@1\ndata: {"error":"%s"}\n\n' "$code" >> "$J"; exit 1; }
  printf 'event: msg.assistant.start@1\ndata: {}\n\nevent: stream@1\ndata: {}\n\nevent: stream@1\ndata: {}\n\nevent: msg.assistant.end@1\ndata: {}\n\n' >> "$J"
done
printf 'event: turn.receipt@1\ndata: {"ok":true}\n\n' >> "$J"
exit 0
STUB
chmod +x "$T/omp"
python3 "$HERE/provider.py" --log "$OUT/provider.jsonl" --ready-file "$T/port" > "$OUT/provider.log" 2>&1 &
PPID_=$!
for _ in $(seq 1 50); do [ -s "$T/port" ] && break; sleep 0.1; done
PORT=$(cat "$T/port"); echo "provider on $PORT"
export STUB_PORT="$PORT"
"$HERE/sample.sh" "$OUT" "$SESS" "$DATA" "$T/omp" 2 > "$OUT/sampler.log" 2>&1 &
SPID=$!
"$HERE/phase.sh" "$OUT" baseline
python3 "$HERE/driver.py" --omp "$T/omp" --out "$OUT" --project "$PROJ" --session-dir "$SESS" --data-dir "$DATA" \
  --duration-min 0.4 --min-turns 6 --max-min 1 --turn-timeout 20 --sentinel SENTINEL-selftest --pause 0.1 > "$OUT/driver.log" 2>&1 &
DPID=$!
sleep 3
for n in 1 2 3 4 5; do
  sleep 1; pid=$(cat "$OUT/omp.pid" 2>/dev/null || true)
  if [ -n "$pid" ]; then kill -9 -- -"$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; fi
  "$HERE/phase.sh" "$OUT" kill n="$n" pid="${pid:-none}"
done
curl -s -X POST -d '{"mode":"stall","seconds":3}' "http://127.0.0.1:$PORT/control" >/dev/null; "$HERE/phase.sh" "$OUT" stall
sleep 5
curl -s -X POST -d '{"mode":"error","status":529}' "http://127.0.0.1:$PORT/control" >/dev/null; "$HERE/phase.sh" "$OUT" error-start
sleep 4
curl -s -X POST -d '{"mode":"normal"}' "http://127.0.0.1:$PORT/control" >/dev/null; "$HERE/phase.sh" "$OUT" error-end
pid=$(cat "$OUT/omp.pid" 2>/dev/null || echo none); "$HERE/phase.sh" "$OUT" sighup pid="$pid"
wait $DPID || true
touch "$OUT/stop-sampler"; wait $SPID || true
"$HERE/phase.sh" "$OUT" final omp_print_procs="$(pgrep -f "^$T/omp print" 2>/dev/null | wc -l | tr -d " ")" envd_procs=0
kill $PPID_ 2>/dev/null || true
python3 "$HERE/report.py" --out "$OUT" --session-dir "$SESS" --min-turns 6 --min-minutes 0.4 --sentinel SENTINEL-selftest || true
echo "self-test artifacts: $T"
