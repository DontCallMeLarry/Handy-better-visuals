#!/usr/bin/env bash
# Create a stable self-signed code-signing identity for local macOS builds.
#
# Why this exists: macOS records TCC grants (Accessibility, Input Monitoring,
# Microphone) against a bundle identifier *plus* a code-signing requirement.
# An ad-hoc signature (`signingIdentity: "-"`, the repo default) has a
# cdhash-based requirement, so every rebuild produces a new identity and the
# grant you just clicked goes stale. Signing with a real certificate makes the
# requirement `identifier "..." and certificate leaf = H"<cert hash>"`, which is
# stable across rebuilds — grant Accessibility once, keep it forever.
#
# Run this once, interactively (it may raise a keychain auth dialog).
set -euo pipefail

CERT_NAME="${1:-Handy Local Dev}"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

# LibreSSL at /usr/bin/openssl emits PKCS#12 that `security import` accepts.
# A Homebrew OpenSSL 3 on PATH would default to AES/PBKDF2, which it does not.
OPENSSL=/usr/bin/openssl

if security find-identity -v -p codesigning | grep -qF "$CERT_NAME"; then
  echo "Code-signing identity already present: $CERT_NAME"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/openssl.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions    = ext
prompt             = no

[dn]
CN = $CERT_NAME

[ext]
basicConstraints     = critical,CA:false
keyUsage             = critical,digitalSignature
extendedKeyUsage     = critical,codeSigning
EOF

echo "Generating self-signed code-signing certificate: $CERT_NAME"
"$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 3650 -sha256 \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/openssl.cnf" 2>/dev/null

"$OPENSSL" pkcs12 -export -out "$WORK/identity.p12" \
  -inkey "$WORK/key.pem" -in "$WORK/cert.pem" -passout pass:

# -A: no ACL on the private key, so codesign never prompts for keychain access.
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "" -A -T /usr/bin/codesign

# codesign only accepts an identity whose certificate is trusted for code
# signing. This is the step that may raise an authentication dialog.
echo "Marking the certificate as trusted for code signing (may prompt)..."
security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$WORK/cert.pem"

if security find-identity -v -p codesigning | grep -qF "$CERT_NAME"; then
  echo
  echo "Ready. \"$CERT_NAME\" is a valid code-signing identity."
  echo "scripts/build-macos-local.sh will pick it up automatically."
else
  echo
  echo "Certificate imported but not reporting as a valid signing identity." >&2
  echo "Open Keychain Access, find \"$CERT_NAME\", and set Trust > Code Signing" >&2
  echo "to \"Always Trust\"." >&2
  exit 1
fi
