#!/usr/bin/env bash
# Sustained kill-9 soak: repeated cycles of load → sync → SIGKILL → restart,
# verifying every earlier cycle's keys after every restart. Catches
# cross-cycle loss (manifest regressions, orphan mishandling) that a single
# kill test cannot.
#
# usage: ./scripts/soak.sh [CYCLES=5] [N=3000]
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
if [[ "$(uname)" == "Darwin" && -d "/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk" && -z "${SDKROOT:-}" ]]; then
  export SDKROOT="/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk"
fi

CYCLES="${1:-5}"
N="${2:-3000}"
SADDR="127.0.0.1:17091"
SDIR="/tmp/fig-soak"
rm -rf "$SDIR"
cargo build -q -p fig-server
SRV=./target/debug/fig-server
BENCH="./target/debug/fig-bench --addr $SADDR --n $N --value-size 64"

start_server() {
  $SRV --dir "$SDIR" --addr "$SADDR" --flush-threshold 262144 > /tmp/fig-soak-srv.log 2>&1 &
  SRV_PID=$!
  for _ in $(seq 1 50); do
    ./target/debug/fig-cli --addr "$SADDR" stats > /dev/null 2>&1 && return 0
    sleep 0.1
  done
  echo "server failed to start"; exit 1
}

start_server
for c in $(seq 1 "$CYCLES"); do
  PREFIX="soak-$(printf '%02d' "$c")-"
  echo "--- cycle $c/$CYCLES: load prefix $PREFIX ---"
  ./target/debug/fig-bench --addr "$SADDR" --n "$N" --value-size 64 --prefix "$PREFIX" | grep -E "PUT|GET"
  kill -9 "$SRV_PID" 2>/dev/null || true
  wait "$SRV_PID" 2>/dev/null || true
  echo "killed with SIGKILL, restarting"
  start_server
  # Every cycle so far must verify: acked data is never lost across restarts.
  for v in $(seq 1 "$c"); do
    VPREFIX="soak-$(printf '%02d' "$v")-"
    ./target/debug/fig-bench --addr "$SADDR" --n "$N" --value-size 64 --prefix "$VPREFIX" --verify-only | grep -E "GET|PASS"
  done
done
echo "--- final state ---"
./target/debug/fig-cli --addr "$SADDR" compact
./target/debug/fig-cli --addr "$SADDR" stats
kill "$SRV_PID" 2>/dev/null || true
wait "$SRV_PID" 2>/dev/null || true
echo "SOAK PASSED: $CYCLES cycles x $N keys, all prefixes verified after every restart"
