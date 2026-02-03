.PHONY: build release install clean test

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

# Install to /usr/local (requires sudo for lib)
install: release
	install -d /usr/local/bin /usr/local/lib
	install target/release/tg /usr/local/bin/
	install target/lib/libtdjson*.dylib /usr/local/lib/
	@echo ""
	@echo "Installed to /usr/local/bin/tg"

# Run tests
test:
	cargo test

# Clean build artifacts
clean:
	cargo clean
