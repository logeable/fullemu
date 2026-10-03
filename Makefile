CARGO ?= cargo
QEMU ?= qemu-system-riscv64
TARGET := riscv64gc-unknown-none-elf
HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')
KERNEL := target/$(TARGET)/release/fullemu

.PHONY: all build run test-fdt clean fmt fmt-check

all: build

build:
	$(CARGO) build --release --target $(TARGET)

run: build
	QEMU="$(QEMU)" ./scripts/run-qemu.sh "$(KERNEL)"

test-fdt:
	$(CARGO) test --lib --target $(HOST_TARGET)

clean:
	$(CARGO) clean

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check
