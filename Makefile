.PHONY: docs docs-clean docs-open

docs:
	./scripts/build-docs.sh

docs-clean:
	rm -rf build/docs

# Serve the built manual locally (requires `make docs` first).
# Uses only the Python standard library; no extra tooling.
docs-open: docs
	python3 -m http.server --directory build/docs 8000
