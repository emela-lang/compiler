#!/bin/sh
set -eu

repo="${EMELA_REPO:-emela-lang/emela}"
install_dir="${EMELA_INSTALL_DIR:-$HOME/.emela/bin}"
version="${EMELA_VERSION:-}"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *)
    echo "unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64 | aarch64)
    if [ "$os" = "apple-darwin" ]; then
      arch="aarch64"
    else
      echo "unsupported architecture for Linux: $(uname -m)" >&2
      exit 1
    fi
    ;;
  x86_64 | amd64)
    if [ "$os" = "unknown-linux-gnu" ]; then
      arch="x86_64"
    else
      echo "unsupported architecture for macOS: $(uname -m)" >&2
      exit 1
    fi
    ;;
  *)
    echo "unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

target="$arch-$os"
api_base="https://api.github.com/repos/$repo/releases"

if [ -n "$version" ]; then
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
  release_json="$(curl -fsSL "$api_base/tags/$tag")"
else
  release_json="$(curl -fsSL "$api_base?per_page=30")"
fi

asset_url="$(printf '%s\n' "$release_json" \
  | awk -v target="$target" '
      /"prerelease": true/ { prerelease = 1 }
      /"prerelease": false/ { prerelease = 0 }
      /"browser_download_url":/ && (prerelease || explicit_tag) && $0 ~ "emela-.*-" target "\\.tar\\.gz" {
        sub(/^.*"browser_download_url": *"/, "")
        sub(/".*$/, "")
        print
        exit
      }
    ' explicit_tag="$([ -n "$version" ] && echo 1 || echo 0)")"

if [ -z "$asset_url" ]; then
  echo "could not find an emela release asset for $target in $repo" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

curl -fsSL "$asset_url" -o "$tmp_dir/emela.tar.gz"
tar -xzf "$tmp_dir/emela.tar.gz" -C "$tmp_dir"

mkdir -p "$install_dir"
find "$tmp_dir" -type f -name emela -perm -u+x -exec cp {} "$install_dir/emela" \; -quit

if [ ! -x "$install_dir/emela" ]; then
  echo "failed to install emela into $install_dir" >&2
  exit 1
fi

echo "installed $("$install_dir/emela" --version) to $install_dir/emela"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "add $install_dir to PATH to run emela directly" ;;
esac
