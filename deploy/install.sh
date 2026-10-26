#!/bin/sh
# Install the Runebook desktop app on Linux.
# The source is private. GitHub SSH access to pinhole-io/root is required.
set -eu

if [ "$(id -u)" -eq 0 ]; then
  echo "error: run the install as your user" >&2
  exit 1
fi

if [ "$(uname -s)" != "Linux" ]; then
  echo "error: this install supports Linux" >&2
  exit 1
fi

echo "Installing Runebook."

if command -v apt-get >/dev/null 2>&1; then
  packages="build-essential pkg-config libssl-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 libvulkan-dev libasound2-dev ffmpeg x11-utils git ca-certificates curl"
  missing=""
  # The package names are a fixed list. Word splitting is intentional.
  # shellcheck disable=SC2086
  for pkg in $packages; do
    if ! dpkg -s "$pkg" >/dev/null 2>&1; then
      missing="$missing $pkg"
    fi
  done
  if [ -n "$missing" ]; then
    if ! command -v sudo >/dev/null 2>&1; then
      echo "error: sudo is required to install packages" >&2
      exit 1
    fi
    sudo env DEBIAN_FRONTEND=noninteractive apt-get update
    # shellcheck disable=SC2086
    sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y $missing
  fi
else
  for cmd in git curl cc pkg-config; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
      echo "error: missing command: $cmd" >&2
      exit 1
    fi
  done
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$work/rustup.sh"
  sh "$work/rustup.sh" -y --default-toolchain stable --profile minimal
fi

if [ -f "${HOME}/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "${HOME}/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is not on PATH" >&2
  exit 1
fi

printf '%s\n' 'github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl' > "$work/known_hosts"
cat > "$work/ssh" <<EOF
#!/bin/sh
exec ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile='$work/known_hosts' "\$@"
EOF
chmod 755 "$work/ssh"
export GIT_SSH_COMMAND="$work/ssh"

src="${XDG_DATA_HOME:-${HOME}/.local/share}/runebook/src"
mkdir -p "$(dirname "$src")"

if [ ! -d "${src}/.git" ]; then
  echo "Cloning the source into ${src}."
  if ! git clone git@github.com:pinhole-io/root.git "$src"; then
    echo "error: GitHub SSH access to pinhole-io/root is required" >&2
    exit 1
  fi
else
  echo "Updating ${src}."
  git -C "$src" fetch origin
  git -C "$src" pull --ff-only
fi

if ! git -C "$src" submodule update --init \
  projects/runebook.sh projects/charlotte projects/superglue projects/notepad; then
  echo "error: GitHub SSH access to the Runebook submodules is required" >&2
  exit 1
fi

echo "Building the release binary. This can take several minutes."
(
  cd "${src}/projects/runebook.sh"
  cargo build --release -p runebook-app
)

bin="${src}/projects/runebook.sh/target/release/runebook"
if [ ! -x "$bin" ]; then
  echo "error: build did not produce ${bin}" >&2
  exit 1
fi

bindir="${HOME}/.local/bin"
mkdir -p "$bindir"
install -m 755 "$bin" "${bindir}/runebook"

case ":${PATH}:" in
  *":${bindir}:"*) ;;
  *)
    echo "Add this directory to PATH: ${bindir}"
    ;;
esac

echo "Installed ${bindir}/runebook"
echo "Research notes need OPENAI_API_KEY and Playwright Chromium."
