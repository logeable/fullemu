CARGO ?= cargo
QEMU ?= qemu-system-riscv64
TARGET := riscv64gc-unknown-none-elf
KERNEL := target/$(TARGET)/release/fullemu

.PHONY: all build run trap-demo clean fmt fmt-check

all: build

build:
	$(CARGO) build --release --target $(TARGET)

run: build
	QEMU="$(QEMU)" ./scripts/run-qemu.sh "$(KERNEL)"

trap-demo:
	QEMU="$(QEMU)" $(CARGO) run --release --target $(TARGET) --features trap-demo

clean:
	$(CARGO) clean

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check
