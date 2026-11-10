#!/bin/sh
# Installs gecko on Linux and macOS: downloads the release binary for this machine,
# checks its SHA-256 against the release's SHA256SUMS and puts it in ~/.local/bin.
#
#   curl -fsSL https://raw.githubusercontent.com/dolusoft/gecko-cli/main/install.sh | sh
#
# GECKO_VERSION=v0.3.0 installs that release instead of the latest one;
# GECKO_INSTALL_DIR changes the directory.
set -eu

repo="dolusoft/gecko-cli"
dir="${GECKO_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
  echo "gecko install: $1" >&2
  exit 1
}

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "no build for $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) fail "no build for $(uname -m)" ;;
esac
asset="gecko-$arch-$os"

if [ -n "${GECKO_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/$GECKO_VERSION"
else
  base="https://github.com/$repo/releases/latest/download"
fi

if command -v curl > /dev/null 2>&1; then
  fetch() { curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget > /dev/null 2>&1; then
  fetch() { wget -q --https-only -O "$2" "$1"; }
else
  fail "curl or wget is needed"
fi

if command -v sha256sum > /dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum > /dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  fail "sha256sum or shasum is needed"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset"
fetch "$base/$asset" "$tmp/$asset" || fail "download failed: $base/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"

expected="$(awk -v name="$asset" '$2 == name { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || fail "$asset is not in SHA256SUMS"
actual="$(sha256 "$tmp/$asset")"
[ "$actual" = "$expected" ] || fail "SHA-256 mismatch for $asset (expected $expected, got $actual)"

mkdir -p "$dir"
chmod 755 "$tmp/$asset"
mv -f "$tmp/$asset" "$dir/gecko"
echo "Installed gecko $("$dir/gecko" --version) to $dir/gecko"

case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    echo
    echo "$dir is not on your PATH. Add this line to your shell profile:"
    echo "  export PATH=\"$dir:\$PATH\""
    ;;
esac
