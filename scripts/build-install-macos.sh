#!/usr/bin/env bash
set -euo pipefail

APP_NAME="Cockpit Tools"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="${INSTALL_DIR:-/Applications}"
SIGN_IDENTITY="${SIGN_IDENTITY:-}"
SKIP_INSTALL="${SKIP_INSTALL:-0}"
AUTO_INSTALL_DEPS="${AUTO_INSTALL_DEPS:-1}"

log() {
  printf '\n==> %s\n' "$*"
}

die() {
  printf 'Error: %s\n' "$*" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

load_common_paths() {
  if [[ -f "$HOME/.cargo/env" ]]; then
    # rustup writes this file and it is safe to source repeatedly.
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi

  if [[ -x /opt/homebrew/bin/brew ]]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
  elif [[ -x /usr/local/bin/brew ]]; then
    eval "$(/usr/local/bin/brew shellenv)"
  fi
}

ensure_xcode_cli() {
  if xcode-select -p >/dev/null 2>&1; then
    return
  fi

  if [[ "$AUTO_INSTALL_DEPS" != "1" ]]; then
    die "Xcode Command Line Tools are required. Run: xcode-select --install"
  fi

  log "Xcode Command Line Tools are required; opening Apple's installer."
  xcode-select --install >/dev/null 2>&1 || true
  die "Complete the Xcode Command Line Tools installation, then run this script again."
}

ensure_homebrew() {
  load_common_paths
  if have brew; then
    return
  fi

  [[ "$AUTO_INSTALL_DEPS" == "1" ]] || die "Homebrew is required to install missing build dependencies."

  log "Installing Homebrew"
  NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  load_common_paths
  have brew || die "Homebrew installation completed but 'brew' is still unavailable."
}

ensure_brew_package() {
  local command_name="$1"
  local formula="$2"

  if have "$command_name"; then
    return
  fi

  ensure_homebrew
  log "Installing $formula with Homebrew"
  brew install "$formula"
  load_common_paths
  have "$command_name" || die "Installed $formula, but '$command_name' is still unavailable."
}

ensure_rust() {
  load_common_paths
  if have cargo && have rustc; then
    return
  fi

  [[ "$AUTO_INSTALL_DEPS" == "1" ]] || die "Rust is required. Install it from https://rustup.rs/"

  log "Installing Rust toolchain with rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs     | sh -s -- -y --profile minimal
  load_common_paths

  have cargo || die "Rust installation completed but 'cargo' is still unavailable."
  have rustc || die "Rust installation completed but 'rustc' is still unavailable."
}

ensure_build_dependencies() {
  ensure_xcode_cli

  have curl || die "The macOS curl command is required."

  ensure_brew_package node node
  have npm || die "Node.js is installed but npm is unavailable."
  ensure_brew_package go go
  ensure_rust

  for command in codesign security xattr ditto osascript find sed; do
    have "$command" || die "Missing required macOS system command: $command"
  done

  log "Build dependencies are ready"
  printf 'Node:  %s\n' "$(node --version)"
  printf 'npm:   %s\n' "$(npm --version)"
  printf 'Go:    %s\n' "$(go version)"
  printf 'Rust:  %s\n' "$(rustc --version)"
  printf 'Cargo: %s\n' "$(cargo --version)"
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

load_common_paths
ensure_build_dependencies

cd "$REPO_ROOT"

log "Installing JavaScript dependencies"
if [[ -f package-lock.json ]]; then
  npm ci
else
  npm install
fi

log "Building the macOS application"
# Build only the .app bundle for local installation. Avoid generating a DMG,
# because this script installs the app directly into /Applications.
# The upstream project also enables updater artifacts for official releases,
# which require the maintainer's Tauri updater private key. Disable them here.
npm run tauri -- build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'

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
