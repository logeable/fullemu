CARGO ?= cargo
QEMU ?= qemu-system-riscv64
OBJCOPY ?= rust-objcopy
LOG_LEVEL ?= info
BOOT_PROGRAM ?= fullemu_user_shell
BOOT_MODE ?= shell
TARGET := riscv64gc-unknown-none-elf
HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')
KERNEL := target/$(TARGET)/release/fullemu
USER_BUILD_DIR := user/target/$(TARGET)/release
USER_IMAGE_MANIFEST := $(USER_BUILD_DIR)/user-programs.manifest
USER_LAYOUT_FILE := $(USER_BUILD_DIR)/user-program-layout
USER_BIN_SOURCES := $(filter-out user/src/bin/disabled-%,$(wildcard user/src/bin/*.rs))
USER_BIN_NAMES := $(patsubst user/src/bin/%.rs,%,$(USER_BIN_SOURCES))
USER_BIN_FLAGS := $(foreach bin,$(USER_BIN_NAMES),--bin $(bin))

.PHONY: all build build-kernel build-user run test-fdt clean fmt fmt-check

all: build

build: build-user
	FULLEMU_USER_IMAGE_MANIFEST=$(USER_IMAGE_MANIFEST) FULLEMU_LOG_LEVEL=$(LOG_LEVEL) FULLEMU_BOOT_PROGRAM=$(BOOT_PROGRAM) FULLEMU_BOOT_MODE=$(BOOT_MODE) $(CARGO) build --release --target $(TARGET)

build-kernel:
	FULLEMU_USER_IMAGE_MANIFEST= FULLEMU_LOG_LEVEL=$(LOG_LEVEL) FULLEMU_BOOT_PROGRAM=$(BOOT_PROGRAM) FULLEMU_BOOT_MODE=$(BOOT_MODE) $(CARGO) build --release --target $(TARGET)

build-user:
	cd user && FULLEMU_USER_LAYOUT_FILE="$(CURDIR)/$(USER_LAYOUT_FILE)" $(CARGO) build --release --target $(TARGET) $(USER_BIN_FLAGS)
	@set -eu; \
	manifest_tmp="$(USER_IMAGE_MANIFEST).tmp"; \
	: > "$$manifest_tmp"; \
	while read -r program_name link_address entry_offset; do \
		program_elf="$(USER_BUILD_DIR)/$$program_name"; \
		program_image="$$program_elf.bin"; \
		$(OBJCOPY) --strip-all -O binary "$$program_elf" "$$program_image"; \
		printf '%s %s %s %s\n' "$$program_name" "$$link_address" "$$entry_offset" "$$program_image" >> "$$manifest_tmp"; \
	done < "$(USER_LAYOUT_FILE)"; \
	mv "$$manifest_tmp" "$(USER_IMAGE_MANIFEST)"

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
