#!/usr/bin/env bash
# Report the memory footprint of the running mithshell daemon.
#
# Private_Dirty is the number that matters: RSS double-counts shared library
# and font pages that are also mapped by every other GTK client.
set -euo pipefail

pid="${1:-}"
if [[ -z "$pid" ]]; then
    pid=$(systemctl --user show -p MainPID --value mithshell.service 2>/dev/null || true)
fi
if [[ -z "$pid" || "$pid" == "0" ]]; then
    pid=$(pgrep -x mithshell | head -1 || true)
fi
[[ -n "$pid" && -d "/proc/$pid" ]] || {
    echo "error: no running mithshell daemon found" >&2
    exit 1
}

echo "Mithshell memory (pid $pid)"
echo

printf '%-18s %s\n' "Metric" "Value"
printf -- '-%.0s' {1..44}
echo

while read -r key value _; do
    case "$key" in
        Rss: | Pss: | Private_Dirty: | Private_Clean: | Shared_Clean:)
            printf '%-18s %8.1f MB\n' "${key%:}" "$(python3 -c "print($value/1024)")"
            ;;
    esac
done < "/proc/$pid/smaps_rollup"

if [[ -r "/proc/$pid/statm" ]]; then
    :
fi

binary=$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)
if [[ -n "$binary" && -f "$binary" ]]; then
    printf '%-18s %8.1f MB\n' "Binary" "$(python3 -c "import os; print(os.path.getsize('$binary')/1048576)")"
fi

echo
echo "Largest mappings (>1 MB RSS)"
printf -- '-%.0s' {1..44}
echo
awk '/^[0-9a-f]/{name=$6} /^Rss:/{rss[name]+=$2} END {for (n in rss) if (rss[n] > 1024) printf "%8.1f MB  %s\n", rss[n]/1024, (n == "" ? "[anonymous]" : n)}' \
    "/proc/$pid/smaps" | sort -rn | head -12

if command -v hyprctl >/dev/null; then
    echo
    echo "Live layer surfaces"
    printf -- '-%.0s' {1..44}
    echo
    hyprctl layers -j | python3 -c '
import json, sys
data = json.load(sys.stdin)
for monitor, value in data.items():
    for layers in value["levels"].values():
        for layer in layers:
            namespace = layer.get("namespace") or ""
            if "mithshell" not in namespace:
                continue
            size = "{}x{}".format(layer["w"], layer["h"])
            print("{:<8} {:<22} {}".format(monitor, namespace, size))
'
fi
