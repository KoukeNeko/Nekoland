#!/bin/sh
# Sets a fan as root, run through pkexec by the menu bar's sensors panel:
#   nekoland-fan mode  /sys/class/hwmon/hwmonN/pwmM <0-9>    writes pwmM_enable
#   nekoland-fan speed /sys/class/hwmon/hwmonN/pwmM <0-255>  sets it by hand, at that speed
# Only a hwmon pwm file can be written, whatever it is asked.
set -eu
[ $# -eq 3 ] || { echo "usage: nekoland-fan mode|speed <pwm> <value>" >&2; exit 2; }
command=$1 pwm=$2 value=$3
case $command in
    mode) most=9 ;;
    speed) most=255 ;;
    *) echo "nekoland-fan: unknown command: $command" >&2; exit 2 ;;
esac
# Taken apart and put back together, so nothing but digits can be in its numbers.
hwmon=${pwm#/sys/class/hwmon/hwmon} chip=${hwmon%%/*} index=${hwmon#*/pwm}
for number in "$chip" "$index"; do
    case $number in
        '' | *[!0-9]*) echo "nekoland-fan: not a hwmon pwm file: $pwm" >&2; exit 2 ;;
    esac
done
[ "$pwm" = "/sys/class/hwmon/hwmon$chip/pwm$index" ] || { echo "nekoland-fan: not a hwmon pwm file: $pwm" >&2; exit 2; }
case $value in
    '' | *[!0-9]* | ????*) echo "nekoland-fan: invalid value: $value" >&2; exit 2 ;;
esac
[ "$value" -le "$most" ] || { echo "nekoland-fan: invalid value: $value" >&2; exit 2; }
[ -e "$pwm" ] && [ -e "${pwm}_enable" ] || { echo "nekoland-fan: $pwm does not exist" >&2; exit 1; }
if [ "$command" = mode ]; then
    echo "$value" > "${pwm}_enable"
else
    echo 1 > "${pwm}_enable"
    echo "$value" > "$pwm"
fi
