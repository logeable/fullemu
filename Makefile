CARGO ?= cargo
QEMU ?= qemu-system-riscv64
READOBJ ?= rust-readobj
PYTHON ?= python3
LOG_LEVEL ?= info
BOOT_PROGRAM ?= init
ifneq ($(origin BOOT_MODE),undefined)
$(error BOOT_MODE 已移除；请使用 BOOT_PROGRAM 指定单个入口程序，并通过 shell 按需运行其他程序)
endif
ifneq ($(origin BATCH_PROGRAMS),undefined)
$(error BATCH_PROGRAMS 已移除；请通过 shell 按需运行用户程序)
endif
ifneq ($(origin BATCH_EXCLUDED_BINS),undefined)
$(error BATCH_EXCLUDED_BINS 已移除；当前会构建用户程序清单中的全部程序)
endif
TARGET := riscv64gc-unknown-none-elf
HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')
KERNEL := target/$(TARGET)/release/fullemu
USER_BUILD_DIR := user/target/$(TARGET)/release
USER_IMAGE_MANIFEST := $(USER_BUILD_DIR)/user-programs.manifest
USER_BIN_SOURCES := $(filter-out user/src/bin/disabled-%,$(wildcard user/src/bin/*.rs))
USER_BIN_NAMES := $(patsubst user/src/bin/%.rs,%,$(USER_BIN_SOURCES))
USER_BIN_FLAGS := $(foreach bin,$(USER_BIN_NAMES),--bin $(bin))

.PHONY: all build build-kernel build-user inspect-kernel-elf run test-fdt clean fmt fmt-check

all: build

build: build-user
	FULLEMU_USER_IMAGE_MANIFEST=$(USER_IMAGE_MANIFEST) FULLEMU_LOG_LEVEL=$(LOG_LEVEL) FULLEMU_BOOT_PROGRAM=$(BOOT_PROGRAM) $(CARGO) build --release --target $(TARGET)

build-kernel:
	FULLEMU_USER_IMAGE_MANIFEST= FULLEMU_LOG_LEVEL=$(LOG_LEVEL) FULLEMU_BOOT_PROGRAM=$(BOOT_PROGRAM) $(CARGO) build --release --target $(TARGET)

build-user:
	cd user && $(CARGO) build --release --target $(TARGET) $(USER_BIN_FLAGS)
	@set -eu; \
	manifest_tmp="$(USER_IMAGE_MANIFEST).tmp"; \
	: > "$$manifest_tmp"; \
	for program_name in $(USER_BIN_NAMES); do \
		program_elf="$(USER_BUILD_DIR)/$$program_name"; \
		printf '%s %s\n' "$$program_name" "$$program_elf" >> "$$manifest_tmp"; \
	done; \
	mv "$$manifest_tmp" "$(USER_IMAGE_MANIFEST)"

# 检查低地址启动入口及高半区 VMA、低地址 LMA 的内核 PT_LOAD 段。
inspect-kernel-elf: build
	$(PYTHON) scripts/check_kernel_elf.py "$(KERNEL)" "$(READOBJ)"

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
