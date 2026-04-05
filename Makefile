.DEFAULT_GOAL := help

CARGO := cargo

.PHONY: help run check build fmt clippy test clean

help: ## Show available commands
	@awk 'BEGIN {FS = ":.*## "}; /^[a-zA-Z0-9_-]+:.*## / {printf "  %-10s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

run: ## Run the app
	$(CARGO) run -p seance-app

check: ## Check the workspace
	$(CARGO) check

build: ## Build the workspace
	$(CARGO) build

fmt: ## Format the workspace
	$(CARGO) fmt

clippy: ## Run clippy with warnings denied
	$(CARGO) clippy --all-targets --all-features -- -D warnings

test: ## Run tests
	$(CARGO) test

clean: ## Remove build artifacts
	$(CARGO) clean
