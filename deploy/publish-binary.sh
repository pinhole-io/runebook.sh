#!/usr/bin/env bash
# Build the release binary for this machine and publish it to the install host.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

case "$(uname -s)" in
  Linux) os="linux" ;;
  Darwin) os="macos" ;;
  *)
    echo "error: publish the binary from Linux or macOS" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="arm64" ;;
  *)
    echo "error: this publisher supports x86_64 and arm64" >&2
    exit 1
    ;;
esac

name="runebook-${os}-${arch}"
if [[ "${RUNEBOOK_SKIP_BUILD:-}" != "1" ]]; then
  echo "Building ${name}."
  cargo build --release -p runebook-app --manifest-path "${ROOT}/Cargo.toml"
fi

target_dir="${CARGO_TARGET_DIR:-${ROOT}/target}"
src="${target_dir}/release/runebook"
if [[ ! -x "${src}" ]]; then
  echo "error: missing ${src}" >&2
  exit 1
fi

mkdir -p "${SCRIPT_DIR}/dist"
cp "${src}" "${SCRIPT_DIR}/dist/${name}"
case "${os}" in
  linux)
    strip --strip-unneeded "${SCRIPT_DIR}/dist/${name}"
    ;;
  macos)
    strip -x "${SCRIPT_DIR}/dist/${name}"
    codesign --force --sign - "${SCRIPT_DIR}/dist/${name}"
    ;;
esac
(
  cd "${SCRIPT_DIR}/dist"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${name}" > "${name}.sha256"
  else
    shasum -a 256 "${name}" > "${name}.sha256"
  fi
)

echo "Checksum:"
cat "${SCRIPT_DIR}/dist/${name}.sha256"

"${SCRIPT_DIR}/setup-server.sh"
