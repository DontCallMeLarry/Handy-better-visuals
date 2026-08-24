#!/usr/bin/env bash
# Install the locally built app to /Applications so macOS will grant it
# Accessibility, and clear the stale TCC records that block the toggle.
#
# macOS records every privacy grant against a bundle identifier *plus* a
# code-signing requirement. An ad-hoc signature pins the binary's cdhash, so
# each rebuild is a different identity as far as TCC is concerned and the grant
# you clicked last time no longer applies — the Accessibility switch appears to
# do nothing. This resets the records so the new build can claim them.
#
# Run scripts/macos-dev-cert.sh once to sign with a stable certificate instead;
# then the grant survives rebuilds and this script preserves it.
#
# Usage:
#   scripts/install-macos-local.sh [--link-models]
#
#   --link-models  Symlink the models directory from an existing
#                  com.pais.handy install, so a rebranded build does not
#                  re-download several hundred MB of models.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONF="$REPO_ROOT/src-tauri/tauri.conf.json"
CERT_NAME="${HANDY_SIGN_IDENTITY:-Handy Local Dev}"
LINK_MODELS=no

for arg in "$@"; do
  case "$arg" in
    --link-models) LINK_MODELS=yes ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

# Read identity from the config rather than hardcoding it — this fork rebrands
# productName/identifier, and a stale hardcoded name would install the wrong app.
read_conf() {
  bun -e "console.log(JSON.parse(require('fs').readFileSync('$CONF','utf8')).$1)"
}
PRODUCT_NAME="$(read_conf productName)"
BUNDLE_ID="$(read_conf identifier)"

APP_SRC="$REPO_ROOT/src-tauri/target/release/bundle/macos/$PRODUCT_NAME.app"
APP_DST="/Applications/$PRODUCT_NAME.app"

if [ ! -d "$APP_SRC" ]; then
  echo "No local build at $APP_SRC" >&2
  echo "Run scripts/build-macos-local.sh first." >&2
  exit 1
fi

# codesign writes its report line-by-line to unbuffered stderr; piping that
# into a reader that exits early (grep -q, awk with exit) races a SIGPIPE,
# which pipefail turns into exit 141 and set -e turns fatal. Capture first,
# parse the variable.
authority_of() {
  local info
  info="$(codesign -dv --verbose=2 "$1" 2>&1 || true)"
  printf '%s\n' "$info" | awk -F'=' '/^Authority=/ {print $2; exit}'
}
plist_of() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null || echo ''
}

echo "Installing $PRODUCT_NAME ($BUNDLE_ID)"

# 1. Stop the running copy — a live process holds the grant being reset.
pkill -f "$PRODUCT_NAME.app/Contents/MacOS/" 2>/dev/null || true
sleep 1

# 2. If the stable identity exists but this build is ad-hoc (certificate created
#    after the build), re-sign in place rather than forcing a full rebuild.
IDENTITIES="$(security find-identity -v -p codesigning 2>/dev/null || true)"
if printf '%s' "$IDENTITIES" | grep -qF "$CERT_NAME" \
   && [ "$(authority_of "$APP_SRC")" != "$CERT_NAME" ]; then
  echo "Re-signing with stable identity: $CERT_NAME"
  codesign --force --deep --options runtime \
    --entitlements "$REPO_ROOT/src-tauri/Entitlements.plist" \
    -s "$CERT_NAME" "$APP_SRC"
fi

LOCAL_AUTHORITY="$(authority_of "$APP_SRC")"
if [ -z "$LOCAL_AUTHORITY" ]; then
  STABLE=no
  LOCAL_AUTHORITY="ad-hoc (no certificate)"
else
  STABLE=yes
fi

# 3. Replace any previous install of *this* app. Refuse if the path is occupied
#    by a different bundle, so a name collision can never eat another app.
PRESERVE_TCC=no
if [ -d "$APP_DST" ]; then
  EXISTING_ID="$(plist_of "$APP_DST" CFBundleIdentifier)"
  if [ "$EXISTING_ID" != "$BUNDLE_ID" ]; then
    echo "$APP_DST already exists and is a different app ($EXISTING_ID)." >&2
    echo "Refusing to overwrite it. Move it aside or rename productName." >&2
    exit 1
  fi
  EXISTING_AUTHORITY="$(authority_of "$APP_DST")"
  echo "Replacing existing install, signed by: ${EXISTING_AUTHORITY:-ad-hoc}"
  if [ "$STABLE" = yes ] && [ "${EXISTING_AUTHORITY:-}" = "$LOCAL_AUTHORITY" ]; then
    echo "Same stable identity — the existing Accessibility grant is preserved."
    PRESERVE_TCC=yes
  fi
  rm -rf "$APP_DST"
fi

cp -R "$APP_SRC" "$APP_DST"
xattr -dr com.apple.quarantine "$APP_DST" 2>/dev/null || true
echo "Installed $APP_DST — signed by: $LOCAL_AUTHORITY"

# 4. Optionally reuse the model cache from a com.pais.handy install. Rebranding
#    the identifier moves the app-support directory, which otherwise means
#    re-downloading every model.
if [ "$LINK_MODELS" = yes ]; then
  SUPPORT="$HOME/Library/Application Support"
  OLD_MODELS="$SUPPORT/com.pais.handy/models"
  NEW_MODELS="$SUPPORT/$BUNDLE_ID/models"
  if [ ! -d "$OLD_MODELS" ]; then
    echo "No models to link at $OLD_MODELS — skipping."
  elif [ -e "$NEW_MODELS" ] && [ ! -L "$NEW_MODELS" ]; then
    echo "$NEW_MODELS already exists and is not a symlink — leaving it alone."
  else
    mkdir -p "$SUPPORT/$BUNDLE_ID"
    ln -sfn "$OLD_MODELS" "$NEW_MODELS"
    echo "Linked models: $NEW_MODELS -> $OLD_MODELS"
  fi
fi

# 5. Clear stale TCC records so the new signature can claim them.
if [ "$PRESERVE_TCC" = no ]; then
  for service in Accessibility ListenEvent Microphone; do
    if tccutil reset "$service" "$BUNDLE_ID" >/dev/null 2>&1; then
      echo "Reset $service for $BUNDLE_ID"
    else
      echo "Could not reset $service (no existing grant, or blocked)"
    fi
  done
fi

open "$APP_DST"

echo
if [ "$PRESERVE_TCC" = no ]; then
  echo "Grant Accessibility (types transcribed text) and Input Monitoring (global"
  echo "shortcut) to $PRODUCT_NAME, then quit and reopen it:"
  echo "  open 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility'"
fi
if [ "$STABLE" = no ]; then
  echo
  echo "Ad-hoc signed: the identity changes on every rebuild, so Accessibility"
  echo "needs re-granting each time. Run scripts/macos-dev-cert.sh once and"
  echo "rebuild to make the grant permanent."
fi
