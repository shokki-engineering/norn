#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Srikanth K
# SPDX-License-Identifier: AGPL-3.0-or-later

# Runs crates/server/tests/disk_full.rs on a filesystem small enough to fill:
# a 32 MiB disk image on macOS (no root), a 32 MiB tmpfs on Linux (sudo).
# The filesystem is removed on exit, whether the test passed or not.
#
# usage: disk-full.sh [CARGO TEST ARGS...]
set -euo pipefail

SIZE_MIB=32
WORK=$(mktemp -d "${TMPDIR:-/tmp}/norn-disk-full.XXXXXX")
MOUNT="$WORK/fs"
mkdir -p "$MOUNT"

case "$(uname -s)" in
Darwin)
    hdiutil create -quiet -size "${SIZE_MIB}m" -fs HFS+ -volname norn-small "$WORK/small.dmg"
    hdiutil attach -quiet -nobrowse -mountpoint "$MOUNT" "$WORK/small.dmg"
    unmount() { hdiutil detach -quiet -force "$MOUNT"; }
    ;;
Linux)
    sudo mount -t tmpfs -o "size=${SIZE_MIB}m,uid=$(id -u),gid=$(id -g),mode=0700" tmpfs "$MOUNT"
    unmount() { sudo umount "$MOUNT"; }
    ;;
*)
    echo "disk-full.sh: no small filesystem on $(uname -s)" >&2
    exit 2
    ;;
esac
trap 'unmount || true; rm -rf "$WORK"' EXIT

# One test at a time: each fills the whole filesystem.
NORN_SMALL_FS="$MOUNT" cargo test --locked -p norn-server --test disk_full "$@" -- \
    --ignored --nocapture --test-threads=1
