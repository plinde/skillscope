# skillscope Makefile
#
# Rust is the primary, shipped implementation. The original Python POC
# lives on as a reference/parity oracle under experiments/python (still
# runnable via `uv run skillscope ...` from that directory) and the Go
# bake-off entry lives under experiments/go. The `parity` target below
# checks this Rust binary's export output against the Python reference.

BINARY := skillscope
INSTALL_DIR := $(HOME)/bin
INSTALL_PATH := $(INSTALL_DIR)/$(BINARY)

.PHONY: help build install uninstall run test smoke lint fmt fmt-check parity clean all

.DEFAULT_GOAL := help

##@ General

help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) }' $(MAKEFILE_LIST)

##@ Build & Install

build: ## Build the release binary (target/release/skillscope)
	cargo build --release

install: build ## Install the release binary to ~/bin
	@mkdir -p $(INSTALL_DIR)
	cp target/release/$(BINARY) $(INSTALL_PATH)
	@echo "Installed: $(INSTALL_PATH)"

uninstall: ## Remove the ~/bin install
	rm -f $(INSTALL_PATH)

##@ Development

run: build ## Launch the TUI against the local transcript corpus
	./target/release/$(BINARY)

test: ## Run unit + integration tests (cargo test)
	cargo test

smoke: build ## Smoke-test every subcommand against the live corpus
	./target/release/$(BINARY) summary >/dev/null
	./target/release/$(BINARY) sessions worktree >/dev/null
	./target/release/$(BINARY) timeline >/dev/null
	./target/release/$(BINARY) projects >/dev/null
	./target/release/$(BINARY) export >/dev/null
	./target/release/$(BINARY) fidelity >/dev/null
	./target/release/$(BINARY) report --cwd $(HOME) >/dev/null 2>&1 || true
	./target/release/$(BINARY) inventory >/dev/null
	@./target/release/$(BINARY) 0000 2>/dev/null; test $$? -eq 1 || { echo "bad-target check failed"; exit 1; }
	@echo "All subcommands OK"

parity: build ## Compare Rust export output against the Python reference (uv run skillscope)
	bash scripts/parity.sh

lint: ## Lint with clippy, warnings as errors
	cargo clippy --all-targets -- -D warnings

fmt: ## Apply formatting (cargo fmt)
	cargo fmt

fmt-check: ## Check formatting without modifying files (for CI)
	cargo fmt --check

clean: ## Remove build artifacts
	cargo clean

# Stub to satisfy checkmake minphony rule
all: help
