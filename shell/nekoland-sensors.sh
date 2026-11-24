#!/bin/bash
# Publishes, for the menu bar's readings, what only root can read, in /run/nekoland-sensors:
#   power             each RAPL domain's power, a line `<zone> <name> <milliwatts>`, averaged
#                     over the second before it; package-0, core, uncore, dram, psys and the like
#   smart-<disk>.json each disk's SMART, as `smartctl -j` gives it, every ten minutes
# Linux keeps RAPL's energy counters to root because read finely enough they tell secrets
# apart (PLATYPUS); a second's average cannot. Run as root by nekoland-sensors.service.
set -u
export LC_ALL=C

out=${1:-/run/nekoland-sensors}
smart_every=600
declare -A energy
last=$EPOCHREALTIME
smart_at=0

while :; do
    now=$EPOCHREALTIME
    elapsed=$(( ${now/./} - ${last/./} ))
    last=$now
    lines=
    for zone in /sys/class/powercap/intel-rapl:*; do
        [ -r "$zone/energy_uj" ] || continue
        read -r name <"$zone/name" || continue
        read -r used <"$zone/energy_uj" || continue
        read -r range <"$zone/max_energy_range_uj" || range=0
        id=${zone##*/}
        if [ -n "${energy[$id]:-}" ] && (( elapsed > 0 )); then
            spent=$(( used - energy[$id] ))
            (( spent < 0 )) && spent=$(( spent + range ))
            # Microjoules over microseconds are watts; a thousand times that, milliwatts.
            lines+="$id $name $(( spent * 1000 / elapsed ))"$'\n'
        fi
        energy[$id]=$used
    done
    printf '%s' "$lines" >"$out/power.new" && mv -f "$out/power.new" "$out/power"

    if (( ${now%.*} - smart_at >= smart_every )) && command -v smartctl >/dev/null; then
        smart_at=${now%.*}
        for disk in /sys/block/nvme*n1 /sys/block/sd?; do
            [ -e "$disk" ] || continue
            name=${disk##*/}
            # A disk that sleeps is left asleep, its last reading kept.
            smartctl -n standby -j -a "/dev/$name" >"$out/smart-$name.new" 2>/dev/null
            if [ -s "$out/smart-$name.new" ]; then
                mv -f "$out/smart-$name.new" "$out/smart-$name.json"
            else
                rm -f "$out/smart-$name.new"
            fi
        done
    fi
    sleep 1
done
