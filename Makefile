.PHONY: help format typecheck fix test test-release python-reference-format \
	python-reference-typecheck python-reference-test differential replay-release \
	sync clean

.DEFAULT_GOAL := help

PYTHON_REFERENCE_DIR ?= reference/python

format: ## Format all Rust workspace crates
	cargo fmt --all

typecheck: ## Lint and type-check all Rust workspace targets
	cargo clippy --workspace --all-targets -- -D warnings

fix: format typecheck ## Format and check the Rust workspace

test: ## Run native Rust workspace tests
	cargo test --workspace

test-release: ## Run Rust tests in release mode with overflow checks enabled
	CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true cargo test --workspace --release

replay-release: ## Build the release replay CLI for differential validation
	cargo build --release --bin cxchange-replay

python-reference-format: ## Format and lint the Python reference
	uv run --directory $(PYTHON_REFERENCE_DIR) --extra dev ruff format .
	uv run --directory $(PYTHON_REFERENCE_DIR) --extra dev ruff check --fix --unsafe-fixes .

python-reference-typecheck: ## Type-check the Python reference
	uv run --directory $(PYTHON_REFERENCE_DIR) --extra dev ty check src/ tests/ ../../tools/differential.py

python-reference-test: ## Run the Python reference test suite
	uv run --directory $(PYTHON_REFERENCE_DIR) --extra dev pytest

differential: replay-release ## Compare the Rust replay CLI with the Python reference
	uv run --directory $(PYTHON_REFERENCE_DIR) --extra dev python ../../tools/differential.py

sync: ## Install Python reference development dependencies
	uv sync --directory $(PYTHON_REFERENCE_DIR) --extra dev

clean: ## Remove Rust and Python generated files
	cargo clean
	rm -rf $(PYTHON_REFERENCE_DIR)/.pytest_cache $(PYTHON_REFERENCE_DIR)/.ruff_cache
	find $(PYTHON_REFERENCE_DIR) -type d -name "__pycache__" -delete

help: ## Display available targets
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  %-28s %s\n", $$1, $$2}'
