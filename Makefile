.PHONY: build release web test e2e fmt clippy linux

# Bound compiler memory by default; operators may explicitly override this.
CARGO_BUILD_JOBS ?= 1
export CARGO_BUILD_JOBS

build:
	cargo build --workspace

release:
	cargo build --release -p wired-magic

web:
	cd web && pnpm install --frozen-lockfile && pnpm build

test:
	cargo test --workspace -- --test-threads=1

e2e:
	scripts/e2e-docker.sh

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

linux:
	docker run --rm -v "$$PWD":/src -w /src -v "$$PWD/.cargo-home":/cargo \
		-e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/src/target-linux -e CARGO_BUILD_JOBS \
		rust:1.98-bookworm cargo build --release -p wired-magic
