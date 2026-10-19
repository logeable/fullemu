#!/bin/sh
# 使用本阶段约定的平台参数运行已构建的内核。
# QEMU 命令行参数统一维护在此脚本中。
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
KERNEL=${1:-"$ROOT_DIR/target/riscv64gc-unknown-none-elf/release/fullemu"}
QEMU=${QEMU:-qemu-system-riscv64}
PYTHON=${PYTHON:-python3}
BLOCK_IMAGE=${BLOCK_IMAGE:-"$ROOT_DIR/target/fullemu-block-test.img"}

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

cd "$ROOT_DIR"
"$PYTHON" "$ROOT_DIR/scripts/create-block-test-image.py" "$BLOCK_IMAGE"
exec "$QEMU" \
    -machine virt \
    -global virtio-mmio.force-legacy=false \
    -cpu rv64 \
    -smp 1 \
    -m 128M \
    -bios default \
    -kernel "$KERNEL" \
    -drive "if=none,file=$BLOCK_IMAGE,format=raw,id=fullemu-block" \
    -device virtio-blk-device,drive=fullemu-block \
    -display none \
    -serial mon:stdio
