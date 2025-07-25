test:
	cargo test

lint:
	cargo clippy --all-features

check:
	cargo clippy --all-features || exit 2
	cargo test || exit 2

run *args:
	cargo run -- {{args}}

run-server *args:
	cargo run -- --server {{args}}

build:
	cargo build --release