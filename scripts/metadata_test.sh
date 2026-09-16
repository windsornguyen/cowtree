#!/usr/bin/env bash
set -euo pipefail

# Linux test data must exercise native reflinks, so allocate one owned Btrfs mount.
# Cargo artifacts stay in the checkout; macOS uses its native APFS temporary directory.
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
case $(uname -s) in
    Darwin)
        exec cargo +1.97.1 test --locked --workspace --all-targets --all-features -- --test-threads=2
        ;;
    Linux) ;;
    *) printf 'FAIL  metadata filesystem tests require Linux or macOS\n' >&2; exit 1 ;;
esac
for command in cargo sudo truncate losetup mount umount mkfs.btrfs; do
    if ! command -v "$command" >/dev/null; then
        printf 'FAIL  missing command: %s\n' "$command" >&2
        exit 1
    fi
done
scratch=$(mktemp -d /var/tmp/cowtree-metadata.XXXXXXXX)
mountpoint="$scratch/mount"
loop_device=''
mounted=0
mkdir "$mountpoint"
cleanup() {
    status=$?
    trap - EXIT INT TERM
    if (( mounted )) && ! sudo umount -- "$mountpoint"; then
        printf 'FAIL  unmount; retained owned files at %s\n' "$scratch" >&2
        exit 1
    fi
    if [[ -n $loop_device ]] && ! sudo losetup --detach "$loop_device"; then
        printf 'FAIL  loop cleanup; retained owned files at %s\n' "$scratch" >&2
        exit 1
    fi
    rm -rf -- "$scratch"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
truncate -s 2G -- "$scratch/btrfs.img"
loop_device=$(sudo losetup --find --show "$scratch/btrfs.img")
sudo mkfs.btrfs -q -f "$loop_device"
sudo mount -- "$loop_device" "$mountpoint"
mounted=1
sudo chown "$(id -u):$(id -g)" "$mountpoint"
TMPDIR="$mountpoint" cargo +1.97.1 test --locked --workspace --all-targets --all-features -- --test-threads=2
