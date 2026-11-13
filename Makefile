.PHONY: build release web test e2e fmt clippy linux

build:
	cargo build --workspace

release:
	cargo build --release -p wired-magic

web:
	cd web && pnpm install --frozen-lockfile && pnpm build

test:
	cargo test --workspace

e2e:
	scripts/e2e-docker.sh

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

linux:
	docker run --rm -v "$$PWD":/src -w /src -v "$$PWD/.cargo-home":/cargo \
		-e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/src/target-linux \
		rust:1.98-bookworm cargo build --release -p wired-magic
