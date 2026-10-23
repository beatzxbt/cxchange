.PHONY: help format typecheck fix test sync clean %

.DEFAULT_GOAL := help

TEST_FLAGS := -xvv -s -p no:anchorpy

format: ## Format code using ruff
	uv run ruff format .
	uv run ruff check --fix --unsafe-fixes .

typecheck: ## Run static type checking
	uv run ty check src/

fix: ## Run all formatters and typecheck
	$(MAKE) format typecheck

test: ## Run all tests (C + Python)
	PYTHONPATH=src uv run pytest $(TEST_FLAGS)

sync: ## Re‑lock and install latest versions
	uv lock --upgrade       # rebuild uv.lock with newer pins
	uv sync --extra dev     # install everything into .venv

clean: ## Remove pytest and ruff caches
	find . -type d -name "__pycache__" -delete
	rm -rf .pytest_cache/ .ruff_cache/

# Pattern rule so additional args do not trigger "No rule to make target"
%:
	@:

# Project‑specific entry points (replace poetry run → uv run)

help: ## Display this help message
	@echo 'Usage:'
	@echo '  make <target>'
	@echo ''
	@echo 'Code Quality:'
	@grep -E '^(format|typecheck|fix):.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ''
	@echo 'Testing:'
	@grep -E '^(test):.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ''
	@echo 'Other:'
	@grep -E '^(sync|clean|help):.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
