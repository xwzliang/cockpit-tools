#!/usr/bin/env bash
set -euo pipefail

APP_NAME="Cockpit Tools"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="${INSTALL_DIR:-/Applications}"
SIGN_IDENTITY="${SIGN_IDENTITY:-}"
SKIP_INSTALL="${SKIP_INSTALL:-0}"

log() {
  printf '\n==> %s\n' "$*"
}

die() {
  printf 'Error: %s\n' "$*" >&2
  exit 1
}

detect_signing_identity() {
  if [[ -n "$SIGN_IDENTITY" ]]; then
    printf '%s\n' "$SIGN_IDENTITY"
    return
  fi

  local identities=""
  local count=0
  local identity=""

  while IFS= read -r identity; do
    [[ -n "$identity" ]] || continue
    count=$((count + 1))
    if [[ -z "$identities" ]]; then
      identities="$identity"
    else
      identities="$identities
$identity"
    fi
  done <<EOF
$(security find-identity -v -p codesigning 2>/dev/null   | sed -n 's/.*"\(Developer ID Application:.*\)".*/\1/p')
EOF

  if [[ "$count" -eq 0 ]]; then
    log "No Developer ID Application identity found; using ad-hoc signing." >&2
    printf '%s\n' "-"
    return
  fi

  if [[ "$count" -gt 1 ]]; then
    printf 'Multiple Developer ID Application identities found:\n%s\n' "$identities" >&2
    die 'Set SIGN_IDENTITY explicitly to choose one.'
  fi

  printf '%s\n' "$identities"
}

[[ "$(uname -s)" == "Darwin" ]] || die "This script only supports macOS."

for command in node npm cargo rustc go codesign security xattr ditto; do
  command -v "$command" >/dev/null 2>&1 || die "Missing required command: $command"
done

if ! xcode-select -p >/dev/null 2>&1; then
  die "Xcode Command Line Tools are required. Run: xcode-select --install"
fi

cd "$REPO_ROOT"

log "Installing JavaScript dependencies"
if [[ -f package-lock.json ]]; then
  npm ci
else
  npm install
fi

log "Building the macOS application"
npm run tauri -- build

APP_BUNDLE="$(find "$REPO_ROOT/src-tauri/target" -type d -path "*/release/bundle/macos/$APP_NAME.app" -print 2>/dev/null | head -n 1 || true)"
if [[ -z "$APP_BUNDLE" ]]; then
  APP_BUNDLE="$(find "$REPO_ROOT/src-tauri/target" -type d -name "$APP_NAME.app" -print 2>/dev/null | head -n 1 || true)"
fi
[[ -n "$APP_BUNDLE" ]] || die "Build succeeded but $APP_NAME.app could not be found under src-tauri/target."

SELECTED_IDENTITY="$(detect_signing_identity)"
log "Signing $APP_BUNDLE"
if [[ "$SELECTED_IDENTITY" == "-" ]]; then
  codesign --force --deep --sign - "$APP_BUNDLE"
else
  log "Using signing identity: $SELECTED_IDENTITY"
  codesign --force --deep --options runtime --timestamp --sign "$SELECTED_IDENTITY" "$APP_BUNDLE"
fi

log "Verifying code signature"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

if [[ "$SKIP_INSTALL" == "1" ]]; then
  log "Build complete (installation skipped)"
  printf '%s\n' "$APP_BUNDLE"
  exit 0
fi

DEST="$INSTALL_DIR/$APP_NAME.app"
log "Installing to $DEST"

osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
sleep 1

install_bundle() {
  rm -rf "$DEST"
  ditto "$APP_BUNDLE" "$DEST"
  xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true
}

if [[ -w "$INSTALL_DIR" ]] || [[ ! -e "$DEST" && -w "$(dirname "$INSTALL_DIR")" ]]; then
  install_bundle
else
  log "Administrator permission is required to write to $INSTALL_DIR"
  sudo rm -rf "$DEST"
  sudo ditto "$APP_BUNDLE" "$DEST"
  sudo xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true
fi

log "Verifying installed application"
codesign --verify --deep --strict --verbose=2 "$DEST"

printf '\nInstalled successfully: %s\n' "$DEST"
printf 'Signing identity: %s\n' "$SELECTED_IDENTITY"
if [[ "$SELECTED_IDENTITY" == "-" ]]; then
  printf 'This is an ad-hoc signed local build. It does not use TestFlight and has no TestFlight expiration.\n'
else
  printf 'Developer ID signing was used. Notarization is not performed by this script.\n'
fi
