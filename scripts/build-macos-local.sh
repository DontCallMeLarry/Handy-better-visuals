#!/usr/bin/env bash
# Build a real Handy.app + DMG for local use on macOS.
#
# Differences from a bare `bun run tauri build`:
#   * createUpdaterArtifacts is forced off — the updater bundle target needs
#     TAURI_SIGNING_PRIVATE_KEY, which only exists in release CI, and bundling
#     fails without it.
#   * only the app and dmg targets are built (no updater tarball).
#   * if the stable local signing identity from scripts/macos-dev-cert.sh
#     exists, both the .app and the DMG are signed with it instead of ad-hoc,
#     so macOS keeps the Accessibility grant across rebuilds.
#
# Output:
#   src-tauri/target/release/bundle/macos/Handy.app
#   src-tauri/target/release/bundle/dmg/Handy_<version>_<arch>.dmg
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CERT_NAME="${HANDY_SIGN_IDENTITY:-Handy Local Dev}"

# rustup puts cargo on PATH from ~/.zshrc, so a non-interactive shell (agent,
# cron, CI) sees no toolchain and tauri fails on `cargo metadata`.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

if [ ! -f src-tauri/resources/models/silero_vad_v4.onnx ]; then
  echo "Missing VAD model. Fetch it first:" >&2
  echo "  mkdir -p src-tauri/resources/models" >&2
  echo "  curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx" >&2
  exit 1
fi

if security find-identity -v -p codesigning | grep -qF "$CERT_NAME"; then
  echo "Signing with stable local identity: $CERT_NAME"
  CONFIG="$(printf '{"bundle":{"createUpdaterArtifacts":false,"macOS":{"signingIdentity":"%s"}}}' "$CERT_NAME")"
else
  echo "No \"$CERT_NAME\" identity found — falling back to ad-hoc signing."
  echo "Accessibility will need re-granting after every rebuild."
  echo "Run scripts/macos-dev-cert.sh once to fix that."
  CONFIG='{"bundle":{"createUpdaterArtifacts":false}}'
fi

# Some macOS toolchains reject the CMake minimum required by the ggml build.
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

# bundle_dmg.sh drives Finder over AppleScript to position the icons in the DMG
# window. That needs Apple Events automation permission for the calling process,
# which a non-interactive shell does not have — it dies with exit 64 and takes
# the whole bundle step with it. Tauri passes --skip-jenkins when CI is set,
# which drops only that cosmetic step. Interactive runs keep the pretty DMG.
if [ ! -t 1 ] && [ -z "${CI:-}" ]; then
  echo "Non-interactive shell: building a plain DMG (no Finder icon layout)."
  export CI=true
fi

# A leftover read-write image from a failed run makes the next attempt fail.
rm -f src-tauri/target/release/bundle/macos/rw.*.dmg

# Intel Macs have no prebuilt ONNX Runtime; link Homebrew's copy dynamically.
if [ "$(uname -m)" = "x86_64" ] && command -v brew >/dev/null 2>&1; then
  export ORT_LIB_LOCATION="${ORT_LIB_LOCATION:-$(brew --prefix onnxruntime)/lib}"
  export ORT_PREFER_DYNAMIC_LINK="${ORT_PREFER_DYNAMIC_LINK:-1}"
fi

bunx tauri build -b app -b dmg --config "$CONFIG" "$@"

echo
echo "Built:"
ls -1d src-tauri/target/release/bundle/macos/*.app 2>/dev/null || true
ls -1 src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null || true
echo
echo "Next: scripts/install-macos-local.sh  (installs to /Applications and resets TCC)"
