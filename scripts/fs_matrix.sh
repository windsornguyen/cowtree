#!/usr/bin/env bash
set -euo pipefail

# Every image, loop device, mount, cache and environment belongs to this run.
if [[ $(uname -s) != Linux || ${EUID} -ne 0 ]]; then
    printf 'FAIL  Linux root privileges required\n' >&2
    exit 1
fi
for command in uv git truncate losetup mount umount mkfs.btrfs mkfs.xfs mkfs.ext4; do
    if ! command -v "$command" >/dev/null; then
        printf 'FAIL  missing command: %s\n' "$command" >&2
        exit 1
    fi
done

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
root=${COWTREE_MATRIX_ROOT:-/var/tmp}
if [[ ! -d $root ]]; then
    printf 'FAIL  matrix root must exist: %s\n' "$root" >&2
    exit 1
fi
scratch=$(mktemp -d "$root/cowtree-matrix.XXXXXXXX")
loop_device=''
mounted=0
mountpoint="$scratch/mount"
mkdir "$mountpoint"
export UV_CACHE_DIR="$scratch/uv-cache"
export UV_PROJECT_ENVIRONMENT="$scratch/venv"
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null

cleanup_mount() {
    if (( mounted )); then
        umount -- "$mountpoint" || return
        mounted=0
    fi
    if [[ -n $loop_device ]]; then
        losetup --detach "$loop_device" || return
        loop_device=''
    fi
}

cleanup() {
    status=$?
    trap - EXIT INT TERM
    if ! cleanup_mount; then
        printf 'FAIL  cleanup; retained owned files at %s\n' "$scratch" >&2
        exit 1
    fi
    rm -rf -- "$scratch"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for filesystem in btrfs xfs-reflink xfs-no-reflink ext4; do
    image="$scratch/$filesystem.img"
    truncate -s 768M -- "$image"
    loop_device=$(losetup --find --show "$image")
    case "$filesystem" in
        btrfs)
            mkfs.btrfs -q -f "$loop_device"
            expected=1
            ;;
        xfs-reflink)
            mkfs.xfs -q -f -m reflink=1 "$loop_device"
            expected=1
            ;;
        xfs-no-reflink)
            mkfs.xfs -q -f -m reflink=0 "$loop_device"
            expected=0
            ;;
        ext4)
            mkfs.ext4 -q -F "$loop_device"
            expected=0
            ;;
    esac
    mount -- "$loop_device" "$mountpoint"
    mounted=1
    if (( expected )); then
        COWTREE_EXPECT_SUPPORTED=1 uv run --locked --no-default-groups --project "$repo" --group test \
            pytest "$repo/tests" "$repo/src" --basetemp "$mountpoint/tests" -q
    else
        COWTREE_EXPECT_SUPPORTED=0 uv run --locked --no-default-groups --project "$repo" --group test \
            pytest "$repo/tests/test_engine.py" --basetemp "$mountpoint/tests" -q \
            -k 'filesystem_support_matches_expectation or unsupported_filesystem_leaves_no_state'
    fi
    cleanup_mount
    rm -- "$image"
    printf '  ok  %s\n' "$filesystem"
done
printf '  4/4 filesystem matrix ok\n'
