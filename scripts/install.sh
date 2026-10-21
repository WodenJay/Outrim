#!/bin/sh
# Keep PATH/HOME literal in generated startup entries and shell instructions.
# shellcheck disable=SC2016
# Install the core only. Agent integration is an explicit `outrim install --agent ...`.
set -eu

version=latest
source_file=
configure_path=1
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version|--from-file)
            [ "$#" -ge 2 ] || { echo "Missing value for $1" >&2; exit 1; }
            case "$1" in --version) version=$2;; --from-file) source_file=$2;; esac
            shift 2 ;;
        --no-path) configure_path=0; shift ;;
        *) printf 'Unknown option: %s\nUsage: sh install.sh [--version <version>] [--from-file <binary>] [--no-path]\n' "$1" >&2; exit 1 ;;
    esac
done
case "$version" in *[!A-Za-z0-9._-]*|'') echo "Invalid version: $version" >&2; exit 1;; esac
[ -n "${HOME:-}" ] || { echo "HOME is not set" >&2; exit 1; }
os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
    Linux/x86_64|Linux/amd64) target=x86_64-unknown-linux-gnu ;;
    Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-gnu ;;
    Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
    *) echo "Unsupported platform: $os/$arch" >&2; exit 1 ;;
esac
asset=outrim-$target
bin_dir=$HOME/.outrim/bin
mkdir -p "$bin_dir"
# Stage on the destination filesystem; a failed download never replaces the old binary.
staging=$(mktemp -d "$bin_dir/.install.XXXXXX")
trap 'rm -rf "$staging"' EXIT HUP INT TERM
if [ -n "$source_file" ]; then
    cp "$source_file" "$staging/outrim"
else
    command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
    base=https://github.com/WodenJay/Outrim/releases
    if [ "$version" = latest ]; then base=$base/latest/download; else base=$base/download/$version; fi
    curl -fsSL --proto '=https' --tlsv1.2 -o "$staging/outrim" "$base/$asset"
    curl -fsSL --proto '=https' --tlsv1.2 -o "$staging/SHA256SUMS" "$base/SHA256SUMS"
    expected=$(awk -v name="$asset" '$2 == name { print $1 }' "$staging/SHA256SUMS")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$staging/outrim" | awk '{print $1}')
    else
        actual=$(shasum -a 256 "$staging/outrim" | awk '{print $1}')
    fi
    if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
        echo "Checksum mismatch: $asset" >&2
        exit 1
    fi
fi
[ -s "$staging/outrim" ] || { echo "Empty binary" >&2; exit 1; }
chmod 755 "$staging/outrim"
mv -f "$staging/outrim" "$bin_dir/outrim"

add_path() {
    profile=$1
    marker='# Outrim user PATH'
    if ! grep -Fqx "$marker" "$profile" 2>/dev/null; then
        printf '\n%s\n%s\n' "$marker" 'case ":$PATH:" in *":$HOME/.outrim/bin:"*) ;; *) export PATH="$HOME/.outrim/bin:$PATH" ;; esac' >> "$profile"
    fi
}
if [ "$configure_path" = 1 ]; then
    add_path "$HOME/.profile"
    case "${SHELL:-}" in
        */bash)
            add_path "$HOME/.bashrc"
            if [ -f "$HOME/.bash_profile" ]; then add_path "$HOME/.bash_profile"
            elif [ -f "$HOME/.bash_login" ]; then add_path "$HOME/.bash_login"; fi ;;
        */zsh) add_path "$HOME/.zprofile"; add_path "$HOME/.zshrc" ;;
        */fish)
            mkdir -p "$HOME/.config/fish/conf.d"
            printf '%s\n' 'fish_add_path "$HOME/.outrim/bin"' > "$HOME/.config/fish/conf.d/outrim.fish" ;;
    esac
fi
printf 'Installed %s\n' "$bin_dir/outrim"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) printf 'Open a new terminal or run:\nexport PATH="$HOME/.outrim/bin:$PATH"\n' ;;
esac
