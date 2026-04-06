.DEFAULT_GOAL := help

CARGO := cargo
RELEASE_DIR ?= dist/release
VERSION ?=

.PHONY: help run run-perf run-perf-expanded run-trace run-perf-trace check build fmt clippy test clean release-version release-notes release-artifacts release-validate release-checksums

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

release-version: ## Print the canonical seance-app release version
	$(CARGO) run -q -p seance-build -- version

release-notes: ## Print release notes for VERSION=<x.y.z>
	@test -n "$(VERSION)" || (echo "VERSION is required" >&2; exit 1)
	$(CARGO) run -q -p seance-build -- release-notes --version "$(VERSION)"

release-artifacts: ## Print canonical release artifact names
	$(CARGO) run -q -p seance-build -- release-artifacts --include-metadata

release-validate: ## Validate release artifacts in RELEASE_DIR=<path>
	$(CARGO) run -q -p seance-build -- validate-release-dir --release-dir "$(RELEASE_DIR)" --include-metadata

release-checksums: ## Write SHA256SUMS.txt for canonical release artifacts in RELEASE_DIR=<path>
	$(CARGO) run -q -p seance-build -- write-checksums --release-dir "$(RELEASE_DIR)" --output "$(RELEASE_DIR)/SHA256SUMS.txt"

clean: ## Remove build artifacts
	$(CARGO) clean
