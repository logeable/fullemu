CARGO ?= cargo
QEMU ?= qemu-system-riscv64
OBJCOPY ?= rust-objcopy
LOG_LEVEL ?= info
TARGET := riscv64gc-unknown-none-elf
HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')
KERNEL := target/$(TARGET)/release/fullemu
USER_BUILD_DIR := user/target/$(TARGET)/release
USER_IMAGE_MANIFEST := $(USER_BUILD_DIR)/user-programs.manifest

.PHONY: all build build-kernel build-user run test-fdt clean fmt fmt-check

all: build

build: build-user
	FULLEMU_USER_IMAGE_MANIFEST=$(USER_IMAGE_MANIFEST) FULLEMU_LOG_LEVEL=$(LOG_LEVEL) $(CARGO) build --release --target $(TARGET)

build-kernel:
	FULLEMU_USER_IMAGE_MANIFEST= FULLEMU_LOG_LEVEL=$(LOG_LEVEL) $(CARGO) build --release --target $(TARGET)

build-user:
	cd user && $(CARGO) build --release --bins --target $(TARGET)
	@set -eu; \
	manifest_tmp="$(USER_IMAGE_MANIFEST).tmp"; \
	set -- user/src/bin/*.rs; \
	if [ ! -f "$$1" ]; then \
		printf 'error: user/src/bin 中没有用户程序源码\n' >&2; \
		exit 1; \
	fi; \
	: > "$$manifest_tmp"; \
	for source in user/src/bin/*.rs; do \
		program_name=$${source##*/}; \
		program_name=$${program_name%.rs}; \
		program_elf="$(USER_BUILD_DIR)/$$program_name"; \
		program_image="$$program_elf.bin"; \
		$(OBJCOPY) --strip-all -O binary "$$program_elf" "$$program_image"; \
		printf '%s %s\n' "$$program_name" "$$program_image" >> "$$manifest_tmp"; \
	done; \
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
