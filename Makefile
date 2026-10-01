.PHONY: docs docs-clean docs-open

docs:
	./scripts/build-docs.sh

docs-clean:
	rm -rf build/docs

# Simple, portable preview: opens the built page in the default browser.
docs-open: docs
	(open build/docs/index.html 2>/dev/null || xdg-open build/docs/index.html 2>/dev/null || true)
