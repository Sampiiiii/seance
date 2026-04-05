.DEFAULT_GOAL := help

CARGO := cargo

.PHONY: help run run-perf run-perf-expanded run-trace run-perf-trace check build fmt clippy test clean

help: ## Show available commands
	@awk 'BEGIN {FS = ":.*## "}; /^[a-zA-Z0-9_-]+:.*## / {printf "  %-10s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

run: ## Run the app
	$(CARGO) run -p seance-app

run-perf: ## Run the app with the compact performance HUD
	SEANCE_PERF_HUD=1 $(CARGO) run -p seance-app

run-perf-expanded: ## Run the app with the expanded performance HUD
	SEANCE_PERF_HUD=expanded $(CARGO) run -p seance-app

run-trace: ## Run the app with tracing enabled
	SEANCE_TRACE=1 $(CARGO) run -p seance-app

run-perf-trace: ## Run the app with the expanded performance HUD and tracing enabled
	SEANCE_PERF_HUD=expanded SEANCE_TRACE=1 $(CARGO) run -p seance-app

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
