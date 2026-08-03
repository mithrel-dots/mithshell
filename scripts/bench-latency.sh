#!/usr/bin/env bash
# Drive the TarraGon launcher with synthetic keystrokes and report the
# end-to-end latency spans collected by the daemon.
#
# The daemon must be running with MITHSHELL_TRACE_LATENCY=1. Keystrokes are
# injected with wtype; timing itself is measured inside the daemon, so
# injection jitter does not pollute the numbers.
set -euo pipefail

MITHSHELL="${MITHSHELL:-mithshell}"
# Delay between characters. Must exceed the debounce so every character
# dispatches its own query, which is the incremental-search case we care about.
DELAY_MS="${DELAY_MS:-350}"
# Time to wait after each query for results to arrive and paint.
SETTLE_MS="${SETTLE_MS:-700}"
ITERATIONS="${ITERATIONS:-2}"

QUERIES=(
    "firefox"
    "kitty"
    "ore"
    "config"
    "42*7"
    "doc"
)

die() {
    echo "error: $*" >&2
    exit 1
}

command -v wtype >/dev/null || die "wtype is required to inject keystrokes"
command -v "$MITHSHELL" >/dev/null || die "$MITHSHELL not found in PATH"

"$MITHSHELL" status >/dev/null 2>&1 || die "mithshell daemon is not running"

enabled=$("$MITHSHELL" latency --json 2>/dev/null | python3 -c \
    'import json,sys; print(json.load(sys.stdin)["data"]["enabled"])' 2>/dev/null || echo False)
if [[ "$enabled" != "True" ]]; then
    die "latency tracing is disabled; restart the daemon with MITHSHELL_TRACE_LATENCY=1 (try: just trace-on)"
fi

sleep_ms() { sleep "$(python3 -c "print($1/1000)")"; }

echo "Driving launcher: ${#QUERIES[@]} queries x ${ITERATIONS} iterations, ${DELAY_MS}ms between keys"

"$MITHSHELL" latency --reset >/dev/null
"$MITHSHELL" search >/dev/null
sleep_ms 600

cleanup() {
    "$MITHSHELL" close --monitor all >/dev/null 2>&1 || true
}
trap cleanup EXIT

for ((iteration = 0; iteration < ITERATIONS; iteration++)); do
    for query in "${QUERIES[@]}"; do
        wtype -d "$DELAY_MS" -- "$query"
        sleep_ms "$SETTLE_MS"
        # Clear the entry one character at a time so the next query starts clean.
        for ((index = 0; index < ${#query}; index++)); do
            wtype -k BackSpace
        done
        sleep_ms 200
    done
done

cleanup
trap - EXIT
sleep_ms 200

echo
"$MITHSHELL" latency
