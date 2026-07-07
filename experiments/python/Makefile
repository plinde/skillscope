# skillscope Makefile (Python POC — experiment, not shipped)
#
# Rust (repo root) is the primary implementation. WARNING: `make install`
# here uses `uv tool install`, which puts a `skillscope` shim on PATH —
# depending on PATH order this can shadow the Rust binary at ~/bin/skillscope.

INSTALL_DIR := $(HOME)/bin
INSTALL_PATH := $(INSTALL_DIR)/skillscope

.PHONY: help build install uninstall check run lint test all clean

.DEFAULT_GOAL := help

##@ General

help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) }' $(MAKEFILE_LIST)

##@ Build & Install

build: ## Build sdist and wheel into dist/
	uv build

install: ## Install skillscope as a uv tool (puts `skillscope` on PATH)
	uv tool install --force --from $(CURDIR) skillscope
	@echo "Installed: $$(command -v skillscope)"

uninstall: ## Remove the uv tool install
	uv tool uninstall skillscope

check: ## Check installation status
	@if command -v skillscope >/dev/null 2>&1; then \
		echo "Status: installed at $$(command -v skillscope)"; \
		skillscope --help | head -3; \
	else \
		echo "Status: not installed (use 'make install' or 'uv run skillscope')"; \
	fi

##@ Development

run: ## Run summary against the local transcript corpus (no install needed)
	uv run skillscope summary

lint: ## Syntax-check all modules
	uv run python3 -m compileall -q src/skillscope

test: ## Smoke-test every subcommand against the live corpus
	uv run skillscope summary >/dev/null
	uv run skillscope sessions worktree >/dev/null
	uv run skillscope timeline --week >/dev/null
	uv run skillscope projects >/dev/null
	uv run skillscope export >/dev/null
	uv run skillscope fidelity >/dev/null
	@echo "All subcommands OK"

clean: ## Remove build artifacts and caches
	rm -rf dist/ src/*.egg-info
	find src -type d -name __pycache__ -exec rm -rf {} +

# Stub to satisfy checkmake minphony rule
all: help
