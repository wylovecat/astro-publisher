#!/usr/bin/env bash
# Assemble a Linux tarball: binaries, an installer, and a desktop entry.
#
# The Windows installer is an NSIS script that writes registry keys and puts
# files in Program Files. None of that exists here, so the Linux equivalent is a
# tarball plus a small shell installer: it copies the binaries into a directory
# on PATH, writes a .desktop file so the app shows up in the launcher, and can
# undo all of it again.
#
# The tarball is built for the machine that runs this script. Cross-compiling
# GPUI is not supported, so "build on the target machine (or a container) and
# copy the tarball over" is the actual distribution flow, which is why the
# install script takes a directory rather than a package.
#
# Usage: packaging/linux/build-tarball.sh [--out DIR]

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
out_dir="$repo_root/dist"

while [ $# -gt 0 ]; do
  case "$1" in
    --out) out_dir="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

version="$(awk -F'"' '/^version[[:space:]]*=/ {print $2; exit}' "$repo_root/Cargo.toml")"
if [ -z "$version" ]; then
  echo "could not read the workspace version from Cargo.toml" >&2
  exit 1
fi

name="astro-publisher"
target="x86_64-unknown-linux-gnu"
[ "$(uname -m)" = "aarch64" ] && target="aarch64-unknown-linux-gnu"

echo "==> building release binaries ($target)"
(cd "$repo_root" && cargo build --release --target "$target" -p astro-publisher -p astro-publisher-core -p astro-publisher-mcp)

bin_dir="$repo_root/target/$target/release"
for binary in astro-publisher astro-publisher-core astro-publisher-mcp; do
  if [ ! -f "$bin_dir/$binary" ]; then
    echo "missing expected binary: $bin_dir/$binary" >&2
    exit 1
  fi
done

echo "==> staging $out_dir/$name-$version-linux"
stage="$out_dir/$name-$version-linux"
# Refuse to package a stale binary: "installed and wrong" is the worst outcome,
# and it looks exactly like success.
for binary in astro-publisher astro-publisher-core astro-publisher-mcp; do
  built="$bin_dir/$binary"
  # Only this binary's own crate (plus the workspace manifests) can make it
  # stale. Comparing against the whole crates/ tree means editing the GUI makes
  # the untouched CLI look stale, and Cargo correctly does not rebuild it -- so
  # the check would refuse to package every time after a single-crate change.
  case "$binary" in
    astro-publisher) crate="$repo_root/crates/app" ;;
    astro-publisher-core) crate="$repo_root/crates/core" ;;
    astro-publisher-mcp) crate="$repo_root/crates/mcp" ;;
  esac
  if [ -n "$(find "$crate" "$repo_root/Cargo.toml" "$repo_root/Cargo.lock" -newer "$built" -print -quit 2>/dev/null)" ]; then
    echo "refusing to package: $binary is older than its source tree" >&2
    exit 1
  fi
done

rm -rf "$stage"
mkdir -p "$stage/bin"
cp "$bin_dir/astro-publisher" "$bin_dir/astro-publisher-core" "$bin_dir/astro-publisher-mcp" "$stage/bin/"
cp "$script_dir/install.sh" "$stage/install.sh"
cp "$script_dir/astro-publisher.desktop.in" "$stage/astro-publisher.desktop"
cp "$script_dir/astro-publisher.svg" "$stage/astro-publisher.svg"
chmod +x "$stage/install.sh" "$stage/bin/"*

# The installer reads this rather than running the binary for `--version`: the
# app is a GUI program, and launching it just to read a version string would
# open a window on the machine being installed.
printf '%s\n' "$version" >"$stage/VERSION"

# Note what this machine was, since the tarball is not portable across distros
# by virtue of carrying a note about itself.
cat >"$stage/BUILD-INFO.txt" <<EOF
$name $version
built: $(uname -s) $(uname -m)  ($(cat /etc/os-release 2>/dev/null | grep '^PRETTY_NAME=' || echo unknown distro))
rustc:  $(rustc --version)
cargo:  $(cargo --version)
target: $target
EOF

(cd "$out_dir" && tar -czf "$name-$version-linux.tar.gz" "$name-$version-linux")

echo "==> $out_dir/$name-$version-linux.tar.gz"
echo "    copy it to the target machine and run: ./install.sh"
