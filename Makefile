.PHONY: build release install clean test

BIN_DIR ?= $(HOME)/bin

# Debug build
build:
	cargo build

# Release build with library setup
release:
	cargo build --release
	mkdir -p target/lib
	cp target/release/build/tdlib-rs-*/out/tdlib/lib/libtdjson*.dylib target/lib/
	@echo ""
	@echo "Release build complete:"
	@echo "  Binary: target/release/tg"
	@echo "  Library: target/lib/libtdjson.dylib"

# Install a symlink to the release binary
install: release
	@mkdir -p "$(BIN_DIR)"
	@ln -sfn "$(PWD)/target/release/tg" "$(BIN_DIR)/tg"
	@echo ""
	@echo "Linked $(BIN_DIR)/tg -> $(PWD)/target/release/tg"

# Run tests
test:
	cargo test

# Clean build artifacts
clean:
	cargo clean
