#!/bin/sh
# 使用本阶段约定的平台参数和可持久化磁盘运行已构建的内核。
# QEMU 命令行参数统一维护在此脚本中。
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
KERNEL=${1:-"$ROOT_DIR/target/riscv64gc-unknown-none-elf/release/fullemu"}
QEMU=${QEMU:-qemu-system-riscv64}
PYTHON=${PYTHON:-python3}
if ! command -v "$QEMU" >/dev/null 2>&1; then
    printf 'error: QEMU executable not found: %s\n' "$QEMU" >&2
    printf 'Install QEMU with RISC-V system emulation, or set QEMU=/path/to/qemu-system-riscv64.\n' >&2
    exit 127
fi

if [ ! -f "$KERNEL" ]; then
    printf 'error: kernel ELF not found: %s\n' "$KERNEL" >&2
    printf 'Build it first with: make build\n' >&2
    exit 2
fi

if [ -n "${BLOCK_IMAGE:-}" ]; then
    case "$BLOCK_IMAGE" in
        /*) FILESYSTEM_IMAGE=$BLOCK_IMAGE ;;
        *) FILESYSTEM_IMAGE=$ROOT_DIR/$BLOCK_IMAGE ;;
    esac
else
    FILESYSTEM_IMAGE=$ROOT_DIR/target/fullemu-filesystem.img
fi

if [ -e "$FILESYSTEM_IMAGE" ] && [ ! -f "$FILESYSTEM_IMAGE" ]; then
    printf 'error: filesystem image path is not a regular file: %s\n' "$FILESYSTEM_IMAGE" >&2
    exit 2
fi
if [ ! -e "$FILESYSTEM_IMAGE" ]; then
    "$PYTHON" "$ROOT_DIR/scripts/create-filesystem-image.py" "$FILESYSTEM_IMAGE"
fi

cd "$ROOT_DIR"
"$QEMU" \
    -machine virt \
    -global virtio-mmio.force-legacy=false \
    -cpu rv64 \
    -smp 1 \
    -m 128M \
    -bios default \
    -kernel "$KERNEL" \
    -drive "if=none,file=$FILESYSTEM_IMAGE,format=raw,id=fullemu-block" \
    -device virtio-blk-device,drive=fullemu-block \
    -display none \
    -serial mon:stdio
