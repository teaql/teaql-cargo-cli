all:
	cargo build --release
	cp target/release/teaql ~/tools

help:
	cargo run -- --help




