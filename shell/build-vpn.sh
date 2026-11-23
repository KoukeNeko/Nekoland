#!/bin/sh
# Builds the VPN helper and its command from third_party/plaitway, as nekoland-vpnd and
# nekoland-vpn, into the folder given: install.sh and CI both build them so. Go fetches
# the toolchain go.mod asks for when the one installed is older.
set -e
out=$(cd "$1" && pwd)
cd "$(dirname "$0")/../third_party/plaitway"
version=$(cat VERSION)
for program in plaitway:nekoland-vpn plaitwayd:nekoland-vpnd; do
    CGO_ENABLED=1 go build -trimpath -ldflags "-s -w -X main.version=$version" \
        -o "$out/${program#*:}" "./cmd/${program%%:*}"
done
