test:
	cargo test

lint:
	cargo clippy --all-features

check: lint test

run *args:
	cargo run -- {{args}}

run-server *args:
	cargo run -- --server {{args}}

build:
	cargo build --release