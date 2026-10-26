#!/bin/sh
# Install the Runebook desktop app on Linux or macOS.
# The script downloads a published binary. It does not clone the source.
set -eu

if [ "$(id -u)" -eq 0 ]; then
  echo "error: run the install as your user" >&2
  exit 1
fi

base="https://runebook.sh"

pick_apt() {
  for name in "$@"; do
    if apt-cache show "$name" >/dev/null 2>&1; then
      echo "$name"
      return 0
    fi
  done
  echo "error: none of these packages are available: $*" >&2
  exit 1
}

install_linux_packages() {
  if ! command -v apt-get >/dev/null 2>&1; then
    for cmd in curl ffmpeg ffprobe; do
      if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "error: missing command: $cmd" >&2
        exit 1
      fi
    done
    return 0
  fi
  if ! command -v sudo >/dev/null 2>&1; then
    echo "error: sudo is required to install packages" >&2
    exit 1
  fi
  sudo env DEBIAN_FRONTEND=noninteractive apt-get update
  alsa="$(pick_apt libasound2t64 libasound2)"
  ssl="$(pick_apt libssl3t64 libssl3)"
  packages="libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 libvulkan1 libxcb1 ${alsa} ${ssl} ffmpeg x11-utils ca-certificates curl"
  missing=""
  # The package names are a fixed list. Word splitting is intentional.
  # shellcheck disable=SC2086
  for pkg in $packages; do
    if ! dpkg -s "$pkg" >/dev/null 2>&1; then
      missing="$missing $pkg"
    fi
  done
  if [ -n "$missing" ]; then
    # shellcheck disable=SC2086
    sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y $missing
  fi
}

install_mac_packages() {
  if command -v ffmpeg >/dev/null 2>&1 && command -v ffprobe >/dev/null 2>&1; then
    return 0
  fi
  if ! command -v brew >/dev/null 2>&1; then
    echo "error: ffmpeg is required. Install Homebrew from https://brew.sh and rerun." >&2
    exit 1
  fi
  brew install ffmpeg
}

case "$(uname -s)" in
  Linux)
    os="linux"
    echo "Installing Runebook on Linux."
    install_linux_packages
    ;;
  Darwin)
    os="macos"
    echo "Installing Runebook on macOS."
    install_mac_packages
    ;;
  *)
    echo "error: this install supports Linux and macOS" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="arm64" ;;
  *)
    echo "error: this install supports x86_64 and arm64" >&2
    exit 1
    ;;
esac

name="runebook-${os}-${arch}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "Downloading ${name}."
if ! curl --proto '=https' --tlsv1.2 -fsSL "${base}/dl/${name}" -o "${work}/${name}"; then
  echo "error: no binary is published for ${os}-${arch}" >&2
  exit 1
fi
if ! curl --proto '=https' --tlsv1.2 -fsSL "${base}/dl/${name}.sha256" -o "${work}/${name}.sha256"; then
  echo "error: no checksum is published for ${os}-${arch}" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$work" && sha256sum -c "${name}.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$work" && shasum -a 256 -c "${name}.sha256")
else
  echo "error: sha256sum or shasum is required" >&2
  exit 1
fi

bindir="${HOME}/.local/bin"
mkdir -p "$bindir"
install -m 755 "${work}/${name}" "${bindir}/runebook"

case ":${PATH}:" in
  *":${bindir}:"*) ;;
  *)
    echo "Add this directory to PATH: ${bindir}"
    ;;
esac

echo "Installed ${bindir}/runebook"
echo "Research notes need OPENAI_API_KEY and Playwright Chromium."
