#!/usr/bin/env bash
set -euo pipefail

DEFAULT_P12_DIR="/Users/broliang/Library/Mobile Documents/com~apple~CloudDocs/keys/apple_developer"
KEYCHAIN="${KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
P12_PATH="${1:-}"

log() {
  printf '\n==> %s\n' "$*"
}

die() {
  printf 'Error: %s\n' "$*" >&2
  exit 1
}

list_developer_identities() {
  security find-identity -v -p codesigning 2>/dev/null     | sed -n 's/.*"\(Developer ID Application:.*\)".*/\1/p'
}

[[ "$(uname -s)" == "Darwin" ]] || die "This script only supports macOS."
command -v security >/dev/null 2>&1 || die "macOS security command not found."

EXISTING="$(list_developer_identities || true)"
if [[ -n "$EXISTING" ]]; then
  log "Developer ID Application identity already installed:"
  printf '%s\n' "$EXISTING"
  exit 0
fi

if [[ -z "$P12_PATH" ]]; then
  [[ -d "$DEFAULT_P12_DIR" ]] || die "P12 directory not found: $DEFAULT_P12_DIR"

  FOUND=""
  COUNT=0
  while IFS= read -r candidate; do
    [[ -n "$candidate" ]] || continue
    COUNT=$((COUNT + 1))
    FOUND="$candidate"
  done <<EOF
$(find "$DEFAULT_P12_DIR" -maxdepth 1 -type f \( -iname '*.p12' -o -iname '*.pfx' \) -print 2>/dev/null)
EOF

  if [[ "$COUNT" -eq 0 ]]; then
    die "No .p12 or .pfx file found in: $DEFAULT_P12_DIR"
  elif [[ "$COUNT" -gt 1 ]]; then
    printf 'Multiple certificate bundles found in %s:\n' "$DEFAULT_P12_DIR" >&2
    find "$DEFAULT_P12_DIR" -maxdepth 1 -type f \( -iname '*.p12' -o -iname '*.pfx' \) -print >&2
    die "Pass the desired file explicitly: $0 '/path/to/certificate.p12'"
  fi

  P12_PATH="$FOUND"
fi

[[ -f "$P12_PATH" ]] || die "P12 file not found: $P12_PATH"

log "Importing signing identity from:"
printf '%s\n' "$P12_PATH"
printf 'Target keychain: %s\n' "$KEYCHAIN"

printf 'P12 password: '
IFS= read -r -s P12_PASSWORD
printf '\n'

security import "$P12_PATH"   -k "$KEYCHAIN"   -P "$P12_PASSWORD"   -T /usr/bin/codesign   -T /usr/bin/security >/dev/null

unset P12_PASSWORD

log "Verifying Developer ID Application identity"
IMPORTED="$(list_developer_identities || true)"
[[ -n "$IMPORTED" ]] || die "Import completed, but no usable Developer ID Application identity was found."

printf '%s\n' "$IMPORTED"
printf '\nSigning setup complete. You can now run:\n'
printf '  ./scripts/build-install-macos.sh\n'
