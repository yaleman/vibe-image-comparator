test:
	cargo test

lint:
	cargo clippy --all-features

check: lint test

run *args:
	cargo run -- {{args}}

build:
	cargo build --release