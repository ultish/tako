#!/usr/bin/env bash
# Build a native macOS tako binary (no container needed — you're already on the
# target platform, unlike scripts/build-tui-rhel9.sh which cross-builds for Linux).
# Plain ratatui app: `cargo build --release` plus packaging to match the RHEL9
# script's dist/ conventions.
#
# Output (repo root):
#   dist/tako-macos-<arch>              # bare Mach-O binary
#   dist/tako-macos-<arch>.tar.gz       # release archive
#   dist/SHA256SUMS                     # merged with any existing entries
#   dist/otool-macos-<arch>.txt         # dynamic-link audit (otool -L)
#
# Usage:
#   ./scripts/build-macos.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DIST="${DIST:-$ROOT/dist}"
mkdir -p "$DIST"

ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
  arm64) ARCH="arm64" ;;
  x86_64) ARCH="x86_64" ;;
  *) ARCH="$ARCH_RAW" ;;
esac

BIN_NAME="tako"
BIN_MACOS_NAME="${BIN_NAME}-macos-${ARCH}"
ARCHIVE_NAME="${BIN_MACOS_NAME}.tar.gz"
BIN_MACOS="$DIST/$BIN_MACOS_NAME"
ARCHIVE="$DIST/$ARCHIVE_NAME"

echo "==> cargo build --release (native $ARCH_RAW)"
cargo build --release

cp "target/release/$BIN_NAME" "$BIN_MACOS"
chmod +x "$BIN_MACOS"

echo
echo "==> dynamic-link audit (otool -L)"
OTOOL_OUT="$DIST/otool-macos-${ARCH}.txt"
otool -L "$BIN_MACOS" | tee "$OTOOL_OUT"
if grep -qiE 'libssl|libcrypto|librdkafka' "$OTOOL_OUT"; then
  echo "warning: binary has unexpected dynamic crypto/rdkafka links:" >&2
  grep -iE 'libssl|libcrypto|librdkafka' "$OTOOL_OUT" >&2
else
  echo "no unexpected crypto/rdkafka dynamic links"
fi

echo
echo "==> packaging"
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/tako-pack.XXXXXX")
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
INNER="${BIN_NAME}-v${VERSION}-macos-${ARCH}"
mkdir -p "$STAGE/$INNER"
cp "$BIN_MACOS" "$STAGE/$INNER/$BIN_NAME"
cat > "$STAGE/$INNER/README.txt" <<EOF
tako v${VERSION} (macOS ${ARCH})

  chmod +x tako
  ./tako --help

Config: ~/.config/tako/config.toml
  (see config.example.toml in the repo)

Requires on PATH (for full features): gradle, skaffold, git.
EOF
cp "$OTOOL_OUT" "$STAGE/$INNER/otool.txt"
tar -C "$STAGE" -czf "$ARCHIVE" "$INNER"
rm -rf "$STAGE"

# Merge into dist/SHA256SUMS rather than clobbering (build-tui-rhel9.sh may have
# already written Linux entries there).
SUMS_TMP="$(mktemp)"
if [[ -f "$DIST/SHA256SUMS" ]]; then
  grep -v -E "$BIN_MACOS_NAME|$ARCHIVE_NAME" "$DIST/SHA256SUMS" > "$SUMS_TMP" || true
fi
(cd "$DIST" && shasum -a 256 "$BIN_MACOS_NAME" "$ARCHIVE_NAME" >> "$SUMS_TMP")
mv "$SUMS_TMP" "$DIST/SHA256SUMS"

echo
echo "==> artifacts"
ls -la "$DIST"
echo
file "$BIN_MACOS" || true
echo
echo "GitHub Release attach (recommended):"
echo "  gh release upload <tag> $ARCHIVE $DIST/SHA256SUMS"
