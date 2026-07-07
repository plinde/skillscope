# skillscope Makefile (Go rewrite — experiment, not shipped)
#
# Rust (repo root) is the primary implementation. This Makefile targets the
# Go module in experiments/go only. WARNING: `make install` writes to
# ~/bin/skillscope, the same path the root Makefile installs to — running
# it will silently overwrite the Rust binary.

MODULE      := github.com/plinde/skillscope
BIN_NAME    := skillscope
BUILD_DIR   := bin
BIN_PATH    := $(BUILD_DIR)/$(BIN_NAME)
INSTALL_DIR := $(HOME)/bin
INSTALL_PATH := $(INSTALL_DIR)/$(BIN_NAME)

.PHONY: help build install uninstall check run lint fmt vet test smoke parity all clean

.DEFAULT_GOAL := help

##@ General

help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) }' $(MAKEFILE_LIST)

##@ Build & Install

build: ## Build the skillscope binary into bin/
	@mkdir -p $(BUILD_DIR)
	go build -o $(BIN_PATH) ./cmd/skillscope
	@echo "Built: $(BIN_PATH)"

install: build ## Install the built binary to ~/bin
	@mkdir -p $(INSTALL_DIR)
	cp $(BIN_PATH) $(INSTALL_PATH)
	@echo "Installed: $(INSTALL_PATH)"

uninstall: ## Remove the installed binary from ~/bin
	rm -f $(INSTALL_PATH)
	@echo "Removed: $(INSTALL_PATH)"

check: ## Check installation status
	@if [ -x "$(INSTALL_PATH)" ]; then \
		echo "Status: installed at $(INSTALL_PATH)"; \
		$(INSTALL_PATH) --help | head -3; \
	else \
		echo "Status: not installed (use 'make install' or 'make run')"; \
	fi

##@ Development

run: build ## Run summary against the local transcript corpus (no install needed)
	$(BIN_PATH) summary

fmt: ## Check gofmt formatting (fails if any file is unformatted)
	@unformatted="$$(gofmt -l .)"; \
	if [ -n "$$unformatted" ]; then \
		echo "Unformatted files:"; echo "$$unformatted"; exit 1; \
	fi
	@echo "gofmt clean"

vet: ## Run go vet
	go vet ./...

lint: fmt vet ## Run gofmt + go vet

test: ## Run unit tests (table-driven, fixture-backed — no live corpus needed)
	go test ./...

smoke: build ## Smoke-test every subcommand against the live ~/.claude/projects corpus
	$(BIN_PATH) summary >/dev/null
	$(BIN_PATH) sessions worktree >/dev/null
	$(BIN_PATH) timeline --week >/dev/null
	$(BIN_PATH) projects >/dev/null
	$(BIN_PATH) export >/dev/null
	$(BIN_PATH) fidelity >/dev/null
	@echo "All subcommands OK"

parity: build ## Diff Go export against the Python reference (uv run skillscope export)
	@cd ../skillscope && uv run skillscope export > /tmp/skillscope-parity-py.ndjson
	@$(BIN_PATH) export > /tmp/skillscope-parity-go.ndjson
	@echo "python: $$(wc -l < /tmp/skillscope-parity-py.ndjson) rows"
	@echo "go:     $$(wc -l < /tmp/skillscope-parity-go.ndjson) rows (includes subagent-origin rows)"

clean: ## Remove build artifacts
	rm -rf $(BUILD_DIR)

# Stub to satisfy checkmake minphony rule
all: help
