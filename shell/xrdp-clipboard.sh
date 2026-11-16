# Keeps the text on Nekoland's clipboard and on that of the X server xrdp drew it in, which
# xrdp shares with its client, the same. Nekoland runs it in an xrdp session.
host="$NEKOLAND_HOST_DISPLAY"
for tool in wl-paste wl-copy xclip; do
    if ! command -v "$tool" >/dev/null; then
        echo "nekoland: no $tool, so the clipboard is not shared with xrdp's client" >&2
        exit 1
    fi
done

# What is copied here goes to xrdp's side at once; xclip keeps it there until replaced.
wl-paste --type text --watch sh -c 'xclip -display "$1" -selection clipboard -in' _ "$host" &

# What is copied on xrdp's side is seen at the next look, and taken unless it is here already.
# It ends with Nekoland, which started it.
last=""
while kill -0 "$PPID" 2>/dev/null && sleep 0.5; do
    there=$(xclip -display "$host" -selection clipboard -out 2>/dev/null) || continue
    [ "$there" = "$last" ] && continue
    last=$there
    [ "$there" = "$(wl-paste --no-newline --type text 2>/dev/null)" ] || printf %s "$there" | wl-copy
done
