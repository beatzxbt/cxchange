.PHONY: help format typecheck fix test test-release replay-release clean

.DEFAULT_GOAL := help

format: ## Format all Rust workspace crates
	cargo fmt --all

typecheck: ## Lint and type-check all Rust workspace targets
	cargo clippy --workspace --all-targets -- -D warnings

fix: format typecheck ## Format and check the Rust workspace

test: ## Run native Rust workspace tests
	cargo test --workspace

test-release: ## Run Rust tests in release mode with overflow checks enabled
	CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true cargo test --workspace --release

replay-release: ## Build the release command replay CLI
	cargo build --release --bin cxchange-replay

clean: ## Remove Rust build output
	cargo clean

help: ## Display available targets
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  %-28s %s\n", $$1, $$2}'
