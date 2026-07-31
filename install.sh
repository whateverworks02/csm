#!/usr/bin/env bash
#
# csm installer - fetches the prebuilt macOS binary from the latest GitHub
# Release, installs it to ~/.local/bin, and runs `csm init` + `csm doctor`.
#
#   curl -fsSL https://raw.githubusercontent.com/whateverworks02/csm/main/install.sh | bash
#
# Re-runnable (idempotent): overwrites the binary; `csm init` is idempotent.
# macOS Apple Silicon only for now (Linux / Intel macOS = future release assets).
set -euo pipefail

OWNER_REPO="${CSM_REPO:-whateverworks02/csm}"
INSTALL_DIR="${CSM_INSTALL_DIR:-$HOME/.local/bin}"

# --- target detection (macOS arm64 only for now) ----------------------------
case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=aarch64-apple-darwin ;;
  Darwin/*)     echo "error: Intel macOS isn't built yet (only Apple Silicon)." >&2; exit 1 ;;
  *)            echo "error: only macOS is supported for now (got $(uname -s)/$(uname -m))." >&2; exit 1 ;;
esac

# --- preflight --------------------------------------------------------------
command -v curl   >/dev/null || { echo "error: curl is required" >&2; exit 1; }
command -v shasum >/dev/null || { echo "error: shasum is required (macOS ships it)" >&2; exit 1; }

TARBALL="csm-${TARGET}.tar.gz"
BASE="https://github.com/${OWNER_REPO}/releases/latest/download"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# --- download + verify checksum ---------------------------------------------
echo "==> downloading ${TARBALL} (latest release)..."
curl -fsSL "${BASE}/${TARBALL}"        -o "${TMP}/${TARBALL}"
curl -fsSL "${BASE}/${TARBALL}.sha256" -o "${TMP}/${TARBALL}.sha256"

echo "==> verifying checksum..."
( cd "$TMP" && shasum -a 256 -c "${TARBALL}.sha256" >/dev/null 2>&1 ) \
  || { echo "error: checksum mismatch - download may be corrupted or tampered" >&2; exit 1; }

# --- install ----------------------------------------------------------------
echo "==> installing to ${INSTALL_DIR}/csm..."
mkdir -p "$INSTALL_DIR"
tar xzf "${TMP}/${TARBALL}" -C "$TMP"
install -m 755 "${TMP}/csm" "${INSTALL_DIR}/csm"
echo "==> installed: $("${INSTALL_DIR}/csm" --version)"

# --- wire up (the SessionStart hook needs `csm` on PATH) --------------------
echo "==> running csm init (SessionStart hook + working-mode prompt)..."
"${INSTALL_DIR}/csm" init

ON_PATH=0
case ":${PATH}:" in *":${INSTALL_DIR}:"*) ON_PATH=1 ;; esac

# --- health check (don't abort if it flags PATH - we hint below) ------------
echo
echo "==> csm doctor:"
if "${INSTALL_DIR}/csm" doctor; then
  DOCTOR_OK=1
else
  DOCTOR_OK=0
fi

if [ "$ON_PATH" -eq 0 ]; then
  echo
  echo "note: ${INSTALL_DIR} is NOT on your PATH."
  echo "  The SessionStart hook runs \`csm hook\`, so add it permanently:"
  echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc   # or ~/.bashrc"
  echo "  then start a new shell and run:  csm <name>"
  echo
  echo "==> installed. Add ${INSTALL_DIR} to PATH (above), then start a session."
elif [ "$DOCTOR_OK" -eq 1 ]; then
  echo
  echo "==> done. start a session with:  csm <name>"
else
  echo
  echo "==> installed, but csm doctor flagged issues (see above). Run \`csm doctor\` for details."
  exit 1
fi
