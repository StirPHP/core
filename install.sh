#!/bin/sh
# Installs stirphp, the StirPHP type checker and language server, on Linux and macOS:
#
#   curl -fsSL https://stirphp.github.io/core/install.sh | sh
#
# Environment:
#   STIRPHP_VERSION      a version such as 0.2.0, or "nightly" (default: the latest release)
#   STIRPHP_INSTALL_DIR  where to put stirphp (default: ~/.local/bin)
#
# Everything is inside main(), so a download cut short can't run half a script.

set -eu

REPO="https://github.com/StirPHP/core/releases"

fail() {
    printf 'stirphp install: %s\n' "$1" >&2
    exit 1
}

has() {
    command -v "$1" >/dev/null 2>&1
}

download() { # url file
    if has curl; then
        curl --proto '=https' --tlsv1.2 -fsSL -o "$2" "$1" || fail "could not download $1"
    elif has wget; then
        wget -q -O "$2" "$1" || fail "could not download $1"
    else
        fail "curl or wget is required"
    fi
}

sha256() { # file
    if has sha256sum; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "sha256sum or shasum is required to verify the download"
    fi
}

target() {
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "there is no build for $(uname -m); see https://github.com/StirPHP/core#readme" ;;
    esac
    case "$(uname -s)" in
        Linux) echo "$arch-unknown-linux-musl" ;;
        Darwin) echo "$arch-apple-darwin" ;;
        MINGW* | MSYS* | CYGWIN*) fail "on Windows, use install.ps1 from PowerShell" ;;
        *) fail "there is no build for $(uname -s); see https://github.com/StirPHP/core#readme" ;;
    esac
}

main() {
    target=$(target)
    version=${STIRPHP_VERSION:-latest}
    version=${version#v}
    dir=${STIRPHP_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}

    case "$version" in
        latest)
            base="$REPO/latest/download"
            archive="stirphp-$target.tar.gz"
            ;;
        nightly)
            base="$REPO/download/nightly"
            archive="stirphp-nightly-$target.tar.gz"
            ;;
        *)
            base="$REPO/download/v$version"
            archive="stirphp-$version-$target.tar.gz"
            ;;
    esac

    has tar || fail "tar is required"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' INT TERM

    printf 'Downloading %s\n' "$archive"
    download "$base/$archive" "$tmp/$archive"
    download "$base/SHA256SUMS" "$tmp/SHA256SUMS"

    want=$(awk -v name="$archive" '$2 == name { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$want" ] || fail "SHA256SUMS doesn't list $archive"
    [ "$want" = "$(sha256 "$tmp/$archive")" ] || fail "checksum mismatch for $archive"

    mkdir -p "$tmp/unpacked"
    tar -xzf "$tmp/$archive" -C "$tmp/unpacked"
    binary=$(find "$tmp/unpacked" -type f -name stirphp | head -n 1)
    [ -n "$binary" ] || fail "$archive has no stirphp"

    # replaced in one step, so this also works while an older stirphp is running
    mkdir -p "$dir"
    cp "$binary" "$dir/.stirphp.$$"
    chmod 755 "$dir/.stirphp.$$"
    mv -f "$dir/.stirphp.$$" "$dir/stirphp"

    printf 'Installed %s to %s\n' "$("$dir/stirphp" --version)" "$dir/stirphp"
    case ":${PATH:-}:" in
        *":$dir:"*) ;;
        *)
            printf '\n%s is not on your PATH. Add this to your shell profile:\n\n' "$dir"
            # shellcheck disable=SC2016  # the literal $PATH is what goes in the profile
            printf '    export PATH="%s:$PATH"\n' "$dir"
            ;;
    esac
}

main
