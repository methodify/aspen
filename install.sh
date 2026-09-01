#!/bin/sh
# Aspen installer (Linux / macOS).
#
#   curl -fsSL https://raw.githubusercontent.com/methodify/aspen/main/install.sh | sh
#
# Installs the latest release binary to ~/.local/bin/aspen (next to claude).
# While the repo is private, set GITHUB_TOKEN (a PAT with repo read access).
#
# Environment:
#   ASPEN_VERSION       tag to install (default: latest)
#   ASPEN_INSTALL_DIR   target dir (default: ~/.local/bin)
#   ASPEN_RELEASE_REPO  owner/repo (default: methodify/aspen)
#   GITHUB_TOKEN        auth token (required while the repo is private)

set -eu

REPO="${ASPEN_RELEASE_REPO:-methodify/aspen}"
API="${ASPEN_GITHUB_API:-https://api.github.com}"
INSTALL_DIR="${ASPEN_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || die "curl is required"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os_part="unknown-linux-gnu" ;;
  Darwin) os_part="apple-darwin" ;;
  *) die "unsupported OS: $os (use install.ps1 on Windows)" ;;
esac
case "$arch" in
  x86_64|amd64)  arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"
asset="aspen-${target}"

auth=""
[ -n "${GITHUB_TOKEN:-}" ] && auth="Authorization: Bearer ${GITHUB_TOKEN}"

fetch_json() {
  if [ -n "$auth" ]; then
    curl -fsSL -H "$auth" -H "Accept: application/vnd.github+json" "$1"
  else
    curl -fsSL -H "Accept: application/vnd.github+json" "$1"
  fi
}

# Resolve the release and its asset API URLs (works for private repos, where
# browser_download_url does not).
if [ -n "${ASPEN_VERSION:-}" ]; then
  tag="$ASPEN_VERSION"
  case "$tag" in v*|[!0-9]*) ;; *) tag="v$tag" ;; esac
  release_url="$API/repos/$REPO/releases/tags/$tag"
else
  release_url="$API/repos/$REPO/releases/latest"
fi
release=$(fetch_json "$release_url") \
  || die "could not fetch release info from $REPO (private repo? set GITHUB_TOKEN)"

tag=$(printf '%s' "$release" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')
[ -n "$tag" ] || die "no release found"

asset_url() {
  # The asset API url for a named asset: the "url" on the object whose
  # "name" matches. Use a real JSON parser when one exists; otherwise a
  # field-order-independent line scrape.
  if command -v python3 >/dev/null 2>&1; then
    printf '%s' "$release" | python3 -c '
import json, sys
want = sys.argv[1]
for a in json.load(sys.stdin).get("assets", []):
    if a.get("name") == want:
        print(a.get("url", "")); break
' "$1"
  elif command -v jq >/dev/null 2>&1; then
    printf '%s' "$release" | jq -r --arg want "$1" \
      '.assets[] | select(.name == $want) | .url'
  else
    # Splitting on '{' puts each asset object's url and name fields on one
    # line (both precede any nested object in GitHub's layout), so a match
    # can be confined to a single object.
    printf '%s' "$release" | tr '{' '\n' | awk -v want="$1" '
      index($0, "\"name\": \"" want "\"") || index($0, "\"name\":\"" want "\"") {
        u=$0
        if (sub(/.*"url"[[:space:]]*:[[:space:]]*"/,"",u) && sub(/".*/,"",u) >= 0 && u ~ /releases\/assets\//) {
          print u; exit
        }
      }
    '
  fi
}

bin_url=$(asset_url "$asset")
sums_url=$(asset_url "SHA256SUMS")
[ -n "$bin_url" ] || die "release $tag has no asset $asset"
[ -n "$sums_url" ] || die "release $tag has no SHA256SUMS"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch_asset() {
  if [ -n "$auth" ]; then
    curl -fsSL -H "$auth" -H "Accept: application/octet-stream" -o "$2" "$1"
  else
    curl -fsSL -H "Accept: application/octet-stream" -o "$2" "$1"
  fi
}

say "downloading aspen $tag ($target) …"
fetch_asset "$bin_url" "$tmp/$asset"
fetch_asset "$sums_url" "$tmp/SHA256SUMS"

expected=$(grep "  $asset\$" "$tmp/SHA256SUMS" | awk '{print $1}')
[ -n "$expected" ] || die "SHA256SUMS has no entry for $asset"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
fi
[ "$actual" = "$expected" ] || die "checksum mismatch: expected $expected, got $actual"

mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/$asset" "$INSTALL_DIR/aspen"
say "installed: $INSTALL_DIR/aspen ($tag)"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say ""; say "note: $INSTALL_DIR is not on your PATH. Add it with:"
     say "  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

say ""
say "get started:  aspen up -d   →  http://127.0.0.1:7420"
