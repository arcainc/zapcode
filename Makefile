.PHONY: build test bench lint format clean

build:
	cargo build

build-release:
	cargo build --release

test:
	cargo test

bench:
	cargo bench -p zapcode-core

lint:
	cargo clippy --all-targets -- -D warnings
	cargo fmt -- --check

format:
	cargo fmt

clean:
	cargo clean
