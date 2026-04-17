.DEFAULT_GOAL := help

CARGO := cargo
APP_PACKAGE := seance-app
APP_RUN := $(CARGO) run -p $(APP_PACKAGE)
RELEASE_DIR ?= dist/release
VERSION ?=
MACOS_SIGNING_ENV_FILE ?= .env.macos-signing
LOG_DIR_DEFAULT := $(HOME)/Library/Application Support/seance/logs
LOG_DIR := $(if $(SEANCE_LOG_DIR),$(SEANCE_LOG_DIR),$(LOG_DIR_DEFAULT))
CRASH_REPORT_DIR := $(HOME)/Library/Logs/DiagnosticReports

define load_macos_signing_env
if [ -f "$(MACOS_SIGNING_ENV_FILE)" ]; then \
	set -a; \
	. ./$(MACOS_SIGNING_ENV_FILE); \
	set +a; \
elif [ -z "$$APPLE_TEAM_ID" ] || [ -z "$$APPLE_DEVELOPMENT_SIGNING_IDENTITY" ] || [ -z "$$APPLE_DEV_PROVISIONING_PROFILE" ]; then \
	echo "Missing macOS signing config. Create $(MACOS_SIGNING_ENV_FILE) from .env.macos-signing.example or export APPLE_TEAM_ID, APPLE_DEVELOPMENT_SIGNING_IDENTITY, and APPLE_DEV_PROVISIONING_PROFILE." >&2; \
	exit 1; \
fi
endef

define resolve_latest_matching_file
latest=""; \
dir="$(1)"; \
for pattern in $(2); do \
	for file in "$$dir"/$$pattern; do \
		if [ ! -f "$$file" ]; then \
			continue; \
		fi; \
		if [ -z "$$latest" ] || [ "$$file" -nt "$$latest" ]; then \
			latest="$$file"; \
		fi; \
	done; \
done; \
if [ -z "$$latest" ]; then \
	echo "$(3)" >&2; \
	exit 1; \
fi
endef

.PHONY: help \
	app-run app-run-perf app-run-perf-expanded \
	debug-run debug-run-perf-expanded debug-lldb \
	signed-build signed-run signed-debug \
	logs-dir logs-latest logs-tail crash-latest \
	check build fmt fmt-check clippy test clean \
	check-public \
	release-version release-notes release-artifacts release-validate release-checksums

help: ## Show available commands
	@awk 'BEGIN {FS = ":.*## "}; /^[a-zA-Z0-9_-]+:.*## / {printf "  %-24s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

app-run: ## Run the app
	$(APP_RUN)

app-run-perf: ## Run the app with the compact performance HUD
	SEANCE_PERF_HUD=1 $(APP_RUN)

app-run-perf-expanded: ## Run the app with the expanded performance HUD
	SEANCE_PERF_HUD=expanded $(APP_RUN)

debug-run: ## Run the app with tracing and full Rust backtraces enabled
	RUST_BACKTRACE=full SEANCE_TRACE=1 $(APP_RUN)

debug-run-perf-expanded: ## Run the app with the expanded performance HUD and diagnostics enabled
	RUST_BACKTRACE=full SEANCE_TRACE=1 SEANCE_PERF_HUD=expanded $(APP_RUN)

debug-lldb: ## Launch the app under LLDB with tracing and full Rust backtraces enabled
	RUST_BACKTRACE=full SEANCE_TRACE=1 lldb -- $(CARGO) run -p $(APP_PACKAGE)

signed-build: ## Build a signed macOS .app bundle for local Touch ID testing
	@$(load_macos_signing_env); \
	./scripts/run-macos-signed-dev.sh --build-only

signed-run: ## Build and launch a signed macOS .app bundle for local Touch ID testing
	@$(load_macos_signing_env); \
	./scripts/run-macos-signed-dev.sh

signed-debug: ## Build and launch a signed macOS .app bundle with tracing and full backtraces enabled
	@$(load_macos_signing_env); \
	RUST_BACKTRACE=full SEANCE_TRACE=1 ./scripts/run-macos-signed-dev.sh

logs-dir: ## Print the effective diagnostics log directory
	@printf '%s\n' "$(LOG_DIR)"

logs-latest: ## Print the newest launch log file path
	@$(call resolve_latest_matching_file,$(LOG_DIR),launch-*.log,No launch logs found in $(LOG_DIR)); \
	printf '%s\n' "$$latest"

logs-tail: ## Tail the newest launch log file
	@$(call resolve_latest_matching_file,$(LOG_DIR),launch-*.log,No launch logs found in $(LOG_DIR)); \
	tail -F "$$latest"

crash-latest: ## Print the newest Seance crash report path from DiagnosticReports
	@$(call resolve_latest_matching_file,$(CRASH_REPORT_DIR),Seance*.ips seance-app*.ips Seance*.crash seance-app*.crash,No Seance crash reports found in $(CRASH_REPORT_DIR)); \
	printf '%s\n' "$$latest"

check: ## Check the workspace
	$(CARGO) check

build: ## Build the workspace
	$(CARGO) build

fmt: ## Format the workspace
	$(CARGO) fmt

fmt-check: ## Check workspace formatting without writing changes
	$(CARGO) fmt --check

clippy: ## Run workspace clippy with warnings denied
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

check-public: ## Fail on unreachable public items in private modules
	RUSTFLAGS="-D unreachable-pub" $(CARGO) check --workspace --all-targets

test: ## Run workspace tests
	$(CARGO) test --workspace

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
