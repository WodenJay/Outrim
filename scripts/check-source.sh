#!/bin/sh
# Prove the distributed source builds without Git metadata, Bun, or node_modules.
set -eu
version=$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)
[ -n "$version" ] || { echo 'Missing package version' >&2; exit 1; }
mkdir -p dist
archive=$(pwd)/dist/outrim-$version-source.tar.gz
git archive --format=tar.gz --prefix="outrim-$version/" --output="$archive" HEAD
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
tar -xzf "$archive" -C "$scratch"
(
    cd "$scratch/outrim-$version"
    cargo test --locked
    cargo build --release --locked
)
printf 'Verified source archive: %s\n' "$archive"
