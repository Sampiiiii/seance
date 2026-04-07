.DEFAULT_GOAL := help

CARGO := cargo
RELEASE_DIR ?= dist/release
VERSION ?=
MACOS_SIGNING_ENV_FILE ?= .env.macos-signing

.PHONY: help run run-perf run-perf-expanded run-trace run-perf-trace run-macos-signed build-macos-signed-app check build fmt clippy test clean release-version release-notes release-artifacts release-validate release-checksums

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

build-macos-signed-app: ## Build a signed macOS .app bundle for local Touch ID testing
	@if [ -f "$(MACOS_SIGNING_ENV_FILE)" ]; then \
		set -a; \
		. ./$(MACOS_SIGNING_ENV_FILE); \
		set +a; \
	elif [ -z "$$APPLE_TEAM_ID" ] || [ -z "$$APPLE_DEVELOPMENT_SIGNING_IDENTITY" ] || [ -z "$$APPLE_DEV_PROVISIONING_PROFILE" ]; then \
		echo "Missing macOS signing config. Create $(MACOS_SIGNING_ENV_FILE) from .env.macos-signing.example or export APPLE_TEAM_ID, APPLE_DEVELOPMENT_SIGNING_IDENTITY, and APPLE_DEV_PROVISIONING_PROFILE." >&2; \
		exit 1; \
	fi; \
	./scripts/run-macos-signed-dev.sh --build-only

run-macos-signed: ## Build and launch a signed macOS .app bundle for local Touch ID testing
	@if [ -f "$(MACOS_SIGNING_ENV_FILE)" ]; then \
		set -a; \
		. ./$(MACOS_SIGNING_ENV_FILE); \
		set +a; \
	elif [ -z "$$APPLE_TEAM_ID" ] || [ -z "$$APPLE_DEVELOPMENT_SIGNING_IDENTITY" ] || [ -z "$$APPLE_DEV_PROVISIONING_PROFILE" ]; then \
		echo "Missing macOS signing config. Create $(MACOS_SIGNING_ENV_FILE) from .env.macos-signing.example or export APPLE_TEAM_ID, APPLE_DEVELOPMENT_SIGNING_IDENTITY, and APPLE_DEV_PROVISIONING_PROFILE." >&2; \
		exit 1; \
	fi; \
	./scripts/run-macos-signed-dev.sh

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
