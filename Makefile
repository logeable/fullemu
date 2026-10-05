CARGO ?= cargo
QEMU ?= qemu-system-riscv64
OBJCOPY ?= rust-objcopy
TARGET := riscv64gc-unknown-none-elf
HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')
KERNEL := target/$(TARGET)/release/fullemu
USER_APP_ELF := user/target/$(TARGET)/release/fullemu_user
USER_APP_BIN := $(USER_APP_ELF).bin

.PHONY: all build build-user run test-fdt clean fmt fmt-check

all: build

build: build-user
	$(CARGO) build --release --target $(TARGET)

build-user:
	cd user && $(CARGO) build --release --target $(TARGET)
	$(OBJCOPY) --strip-all -O binary $(USER_APP_ELF) $(USER_APP_BIN)

run: build
	QEMU="$(QEMU)" ./scripts/run-qemu.sh "$(KERNEL)"

test-fdt:
	$(CARGO) test --lib --target $(HOST_TARGET)

clean:
	$(CARGO) clean
	cd user && $(CARGO) clean

fmt:
	$(CARGO) fmt --all
	cd user && $(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check
	cd user && $(CARGO) fmt --all -- --check
