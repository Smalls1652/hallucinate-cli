# Define package variables
PACKAGE_NAME := hallucinate-cli
PACKAGE_BINARY := hallucinate

# Define project directory variables
ARTIFACTS_DIR := artifacts
RUST_TARGET_DIR := target

# Define platform variables
ifeq ($(OS),Windows_NT)
	PLATFORM_OS := win

	ifeq ($(PROCESSOR_ARCHITECTURE),AMD64)
		PLATFORM_ARCH := amd64
	endif

	ifeq ($(PROCESSOR_ARCHITECTURE),ARM64)
		PLATFORM_ARCH := arm64
	endif
else
	UNAME_S := $(shell uname -s)
	
	ifeq ($(UNAME_S),Linux)
		PLATFORM_OS := linux
	endif

	ifeq ($(UNAME_S),Darwin)
		PLATFORM_OS := macos
	endif

	UNAME_M := $(shell uname -m)
	
	ifeq ($(UNAME_M),x86_64)
		PLATFORM_ARCH := amd64
	endif

	ifeq ($(UNAME_M),arm64)
		PLATFORM_ARCH := arm64
	endif
endif

ifeq ($(PLATFORM_OS),linux)
	ifeq ($(PLATFORM_ARCH),amd64)
		RUST_TARGET := x86_64-unknown-linux-gnu

		# Setting and exporting compile flags for cargo to make
		# life a little easier for cross-compilation.
		CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER := x86_64-linux-gnu-gcc
		CC_x86_64_unknown_linux_gnu := x86_64-linux-gnu-gcc
		CXX_x86_64_unknown_linux_gnu := x86_64-linux-gnu-g++

		export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER
		export CC_x86_64_unknown_linux_gnu
		export CXX_x86_64_unknown_linux_gnu
	endif

	ifeq ($(PLATFORM_ARCH),arm64)
		RUST_TARGET := aarch64-unknown-linux-gnu

		# Setting and exporting compile flags for cargo to make
		# life a little easier for cross-compilation.
		CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER := aarch64-linux-gnu-gcc
		CC_aarch64_unknown_linux_gnu := aarch64-linux-gnu-gcc
		CXX_aarch64_unknown_linux_gnu := aarch64-linux-gnu-g++

		export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER
		export CC_aarch64_unknown_linux_gnu
		export CXX_aarch64_unknown_linux_gnu
	endif
endif

ifeq ($(PLATFORM_OS),macos)
	ifeq ($(PLATFORM_ARCH),amd64)
		RUST_TARGET := x86_64-apple-darwin
	endif

	ifeq ($(PLATFORM_ARCH),arm64)
		RUST_TARGET := aarch64-apple-darwin
	endif
endif

ifeq ($(PLATFORM_OS),win)
	ifeq ($(PLATFORM_ARCH),amd64)
		RUST_TARGET := x86_64-pc-windows-gnu
	endif

	ifeq ($(PLATFORM_ARCH),arm64)
		RUST_TARGET := aarch64-pc-windows-gnu
	endif
endif

# Shows the config.
show-config:
	@echo "PACKAGE_NAME = $(PACKAGE_NAME)"
	@echo "PACKAGE_BINARY = $(PACKAGE_BINARY)"
	@echo "PLATFORM_OS = $(PLATFORM_OS)"
	@echo "PLATFORM_ARCH = $(PLATFORM_ARCH)"
	@echo "RUST_TARGET = $(RUST_TARGET)"
	@echo "RUST_TARGET_DIR = $(RUST_TARGET_DIR)"
	@echo "ARTIFACTS_DIR = $(ARTIFACTS_DIR)"

	@if [ "$(PLATFORM_OS)" == "linux" ]; then \
		echo ""; \
		echo "Linux specific flags:"; \
		if [ "$(PLATFORM_ARCH)" == "amd64" ]; then \
			echo "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = $(CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER)"; \
			echo "CC_x86_64_unknown_linux_gnu = $(CC_x86_64_unknown_linux_gnu)"; \
			echo "CXX_x86_64_unknown_linux_gnu = $(CXX_x86_64_unknown_linux_gnu)"; \
		elif [ "$(PLATFORM_ARCH)" == "arm64" ]; then \
			echo "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = $(CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER)"; \
			echo "CC_aarch64_unknown_linux_gnu = $(CC_aarch64_unknown_linux_gnu)"; \
			echo "CXX_aarch64_unknown_linux_gnu = $(CXX_aarch64_unknown_linux_gnu)"; \
		fi; \
	fi

# Perform a build.
build:
	@cargo build

# Perform a nightly Rust build.
build-nightly:
	@cargo +nightly build

# Perform a nightly Rust build.
build-nightly-backtrace:
	@RUSTFLAGS="-Zmacro-backtrace" cargo +nightly build

# Perform a build with verbose output.
build-verbose:
	@cargo build -vv

# Perform a release build.
build-release:
	@cargo build --target=$(RUST_TARGET) --release --package=${PACKAGE_NAME} --locked

# Build a release build and copy it to the artifacts directory.
build-release-artifacts:
	$(MAKE) build-release

	@if [ -d $(ARTIFACTS_DIR) ]; then \
		echo "Removing existing artifacts directory -> $(ARTIFACTS_DIR)"; \
		rm -rf $(ARTIFACTS_DIR); \
	fi
	
	@mkdir -p $(ARTIFACTS_DIR)/$(PLATFORM_OS)_$(PLATFORM_ARCH)
	
	@echo "target/$(RUST_TARGET)/release/$(PACKAGE_BINARY) -> artifacts/$(PLATFORM_OS)_$(PLATFORM_ARCH)/"
	@cp $(RUST_TARGET_DIR)/$(RUST_TARGET)/release/$(PACKAGE_BINARY) $(ARTIFACTS_DIR)/$(PLATFORM_OS)_$(PLATFORM_ARCH)/

# Perform a release install from the repo.
install:
	@cargo install --path "./crates/hallucinate-cli" --locked

# Clean the Rust target directory.
clean:
	@echo "Cleaning target directory -> $(RUST_TARGET_DIR)/"
	@cargo clean --workspace
	@rm -rf $(RUST_TARGET_DIR)

# Format all code in the project.
format:
	@cargo +nightly fmt -- --emit files

# Check for errors.
check:
	@cargo check

# Check for errors (Nightly).
check-nightly:
	@cargo +nightly check
