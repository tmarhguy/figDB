#!/usr/bin/env bash
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
# macOS workaround: MacOSX27.0 SDK ships malformed .tbd files for arm64e.x1 that
# break `cc` linking (tapi error). Pin to the last known-good SDK when present.
if [[ "$(uname)" == "Darwin" && -d "/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk" && -z "${SDKROOT:-}" ]]; then
  export SDKROOT="/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk"
fi
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
