.PHONY: build test fmt clippy serve docker up clean

build:
	cargo build --release

test:
	cargo test --all

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets -- -D warnings

serve:
	cargo run --release -- serve --config sparql-mcp.toml

docker:
	docker compose build

up:
	docker compose up -d

clean:
	cargo clean
