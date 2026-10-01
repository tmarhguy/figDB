#!/usr/bin/env bash
# The real test: on-disk proof with numbers + a genuine `kill -9`.
#
# 1. Fresh harness run (HashMap oracle, throughput, restart, deletes) + du/ls.
# 2. Second process re-verifies the same directory after exit.
# 3. Fresh writer child is SIGKILLed mid-write; verifier checks the acked
#    prefix survived and the DB opens clean.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
if [[ "$(uname)" == "Darwin" && -d "/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk" && -z "${SDKROOT:-}" ]]; then
  export SDKROOT="/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk"
fi

DIR="${1:-/tmp/fig-real}"
N="${2:-20000}"
CDIR="${3:-/tmp/fig-crash}"

echo "=== 1. fresh run: $N keys into $DIR (vs HashMap oracle) ==="
cargo run -q -p fig-storage --example real -- "$DIR" "$N"
echo
echo "--- on-disk layout ---"
ls -R "$DIR" | head -n 20
du -sh "$DIR"
echo
echo "=== 2. cross-process verify (new process, same dir) ==="
cargo run -q -p fig-storage --example real -- "$DIR" "$N" verify
echo
echo "=== 3. real SIGKILL test into $CDIR ==="
rm -rf "$CDIR"
cargo build -q -p fig-storage --example crash_child --example verify_crash
./target/debug/examples/crash_child "$CDIR" > /tmp/fig-crash-child.log 2>&1 &
CHILD=$!
# Wait for the first synced batch to land.
for i in $(seq 1 50); do
  [[ -f "$CDIR/SYNCED" ]] && break
  sleep 0.1
done
sleep 0.5
kill -9 "$CHILD" 2>/dev/null || true
wait "$CHILD" 2>/dev/null || true
echo "killed writer (pid $CHILD) with SIGKILL"
tail -n 3 /tmp/fig-crash-child.log || true
cargo run -q -p fig-storage --example verify_crash -- "$CDIR"
echo
echo "=== 4. server over TCP: CLI writes, SIGKILL, restart ==="
SADDR="127.0.0.1:17071"
SDIR="/tmp/fig-srv"
rm -rf "$SDIR"
cargo build -q -p fig-server
./target/debug/fig-server --dir "$SDIR" --addr "$SADDR" > /tmp/fig-srv-test.log 2>&1 &
SRV=$!
CLI="./target/debug/fig-cli --addr $SADDR"
for i in $(seq 1 50); do
  $CLI stats > /dev/null 2>&1 && break
  sleep 0.1
done
$CLI put hello world
$CLI put foo bar
$CLI sync
test "$($CLI get hello)" = "world"
kill -9 "$SRV" 2>/dev/null || true
wait "$SRV" 2>/dev/null || true
echo "killed server (pid $SRV) with SIGKILL"
./target/debug/fig-server --dir "$SDIR" --addr "$SADDR" > /tmp/fig-srv-test2.log 2>&1 &
SRV=$!
for i in $(seq 1 50); do
  $CLI stats > /dev/null 2>&1 && break
  sleep 0.1
done
test "$($CLI get hello)" = "world"
test "$($CLI get foo)" = "bar"
$CLI stats
kill "$SRV" 2>/dev/null || true
wait "$SRV" 2>/dev/null || true
echo "server survived SIGKILL with acked writes intact"
echo
echo "ALL REAL TESTS PASSED"
