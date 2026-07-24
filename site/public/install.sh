#!/bin/sh
# Install the latest testless release binary.
#
#   curl -fsSL https://raw.githubusercontent.com/itaywol/testless/main/install.sh | sh
#
# Override the install location with TESTLESS_INSTALL_DIR (default: ~/.local/bin).
set -eu

repo="itaywol/testless"
install_dir="${TESTLESS_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *)
        echo "error: unsupported OS: $os" >&2
        exit 1
        ;;
esac

case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *)
        echo "error: unsupported architecture: $arch" >&2
        exit 1
        ;;
esac

target="${arch_part}-${os_part}"
url="https://github.com/${repo}/releases/latest/download/testless-${target}.tar.gz"

if ! command -v curl >/dev/null 2>&1; then
    echo "error: curl is required" >&2
    exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

echo "Downloading testless for ${target}..."
if ! curl -fsSL "$url" -o "$tmp_dir/testless.tar.gz"; then
    echo "error: failed to download $url" >&2
    echo "(no prebuilt binary for ${target}? check https://github.com/${repo}/releases)" >&2
    exit 1
fi

tar xzf "$tmp_dir/testless.tar.gz" -C "$tmp_dir" testless

mkdir -p "$install_dir"
mv "$tmp_dir/testless" "$install_dir/testless"
chmod +x "$install_dir/testless"

echo "Installed testless to ${install_dir}/testless"

case ":$PATH:" in
    *":${install_dir}:"*) ;;
    *)
        echo
        echo "${install_dir} is not on your PATH. Add it with:"
        echo "  export PATH=\"${install_dir}:\$PATH\""
        ;;
esac
