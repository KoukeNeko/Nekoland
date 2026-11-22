#!/bin/sh
# Builds the Windows program that asks a Wine bottle's programs for their right-click
# items, into the folder given: install.sh and CI both build it so.
set -e
src=$(dirname "$0")/..
x86_64-w64-mingw32-gcc -O2 -Wall -municode -s -o "$1/nekoland-shellmenu.exe" \
    "$src/ui/src/windows/shellmenu.c" -lole32 -lshell32 -luuid
