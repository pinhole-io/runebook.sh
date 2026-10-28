#!/usr/bin/env bash
# Install Caddy on the install host and publish deploy/install.sh.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_FILE="${HOME}/.runebook/install-host.env"
DOMAIN="${RUNEBOOK_DOMAIN:-runebook.sh}"

if [[ ! -f "${STATE_FILE}" ]]; then
  echo "error: missing ${STATE_FILE}. Run ./deploy/provision.sh first." >&2
  exit 1
fi

# shellcheck disable=SC1090
source "${STATE_FILE}"
DOMAIN="${RUNEBOOK_DOMAIN:-${DOMAIN}}"
REMOTE="${1:-${RUNEBOOK_EC2_USER:-ubuntu}@${RUNEBOOK_EC2_PUBLIC_IP}}"
EIP="${RUNEBOOK_EC2_PUBLIC_IP:-}"

if [[ -z "${EIP}" ]]; then
  echo "error: RUNEBOOK_EC2_PUBLIC_IP is empty in ${STATE_FILE}" >&2
  exit 1
fi

SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10)
SCP=(scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10)

echo "Waiting for SSH on ${REMOTE}..."
ready=0
for _ in $(seq 1 30); do
  if "${SSH[@]}" "${REMOTE}" true >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 5
done
if [[ "${ready}" -ne 1 ]]; then
  echo "error: SSH to ${REMOTE} failed" >&2
  exit 1
fi

echo "Installing Caddy..."
"${SSH[@]}" "${REMOTE}" bash -s <<'REMOTE_DEPS'
set -euo pipefail
sudo apt-get update -qq
sudo apt-get install -y -qq curl ca-certificates debian-keyring debian-archive-keyring apt-transport-https
if ! command -v caddy >/dev/null 2>&1; then
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
  sudo apt-get update -qq
  sudo apt-get install -y -qq caddy
fi
sudo mkdir -p /srv/runebook
REMOTE_DEPS

CADDY_TMP="$(mktemp)"
sed "s|runebook.sh|${DOMAIN}|g" "${SCRIPT_DIR}/Caddyfile" > "${CADDY_TMP}"

"${SCP[@]}" "${SCRIPT_DIR}/install.sh" "${REMOTE}:/tmp/install.sh"
"${SCP[@]}" "${SCRIPT_DIR}/uninstall.sh" "${REMOTE}:/tmp/uninstall.sh"
"${SCP[@]}" "${CADDY_TMP}" "${REMOTE}:/tmp/Caddyfile"
rm -f "${CADDY_TMP}"

if compgen -G "${SCRIPT_DIR}/dist/*" >/dev/null; then
  "${SSH[@]}" "${REMOTE}" "rm -rf /tmp/runebook-dl && mkdir -p /tmp/runebook-dl"
  "${SCP[@]}" "${SCRIPT_DIR}/dist/"* "${REMOTE}:/tmp/runebook-dl/"
fi

"${SSH[@]}" "${REMOTE}" bash -s <<'REMOTE_INSTALL'
set -euo pipefail
sudo install -m 644 /tmp/install.sh /srv/runebook/install.sh
sudo install -m 644 /tmp/uninstall.sh /srv/runebook/uninstall.sh
rm -f /tmp/install.sh /tmp/uninstall.sh
if [ -d /tmp/runebook-dl ]; then
  sudo mkdir -p /srv/runebook/dl
  sudo install -m 644 /tmp/runebook-dl/* /srv/runebook/dl/
  rm -rf /tmp/runebook-dl
fi
sudo install -m 640 -o root -g caddy /tmp/Caddyfile /etc/caddy/Caddyfile
rm -f /tmp/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
sudo systemctl enable caddy
if ! sudo systemctl restart caddy; then
  sudo journalctl -u caddy -n 40 --no-pager >&2 || true
  exit 1
fi
REMOTE_INSTALL

resolve_a() {
  if command -v dig >/dev/null 2>&1; then
    dig +short "$1" A | head -1
  else
    getent ahostsv4 "$1" | awk '{print $1; exit}'
  fi
}

RESOLVED="$(resolve_a "${DOMAIN}" || true)"
if [[ "${RESOLVED}" != "${EIP}" ]]; then
  echo ""
  echo "The host is ready. HTTPS starts after DNS points at it."
  echo "At the registrar, set this record and turn off the parking page:"
  echo "  ${DOMAIN}  A  ${EIP}"
  if [[ -n "${RESOLVED}" ]]; then
    echo "The current address is ${RESOLVED}."
  fi
  echo "Then run ./deploy/setup-server.sh again to verify the certificate."
  exit 0
fi

echo "DNS ${DOMAIN} -> ${EIP}. Waiting for the certificate..."
ok=0
for _ in $(seq 1 20); do
  body="$(curl -fsSL --max-time 15 -A "curl/8.0.0" "https://${DOMAIN}" || true)"
  if [[ "${body}" == "#!/"* ]]; then
    ok=1
    break
  fi
  sleep 3
done

if [[ "${ok}" -ne 1 ]]; then
  echo "error: https://${DOMAIN} did not return the install script" >&2
  "${SSH[@]}" "${REMOTE}" "sudo journalctl -u caddy -n 40 --no-pager" >&2 || true
  exit 1
fi

page="$(curl -fsSL --max-time 15 -A "Mozilla/5.0" "https://${DOMAIN}")"
if [[ "${page}" != *"<title>Runebook</title>"* ]] || [[ "${page}" != *"curl -fsSL https://${DOMAIN} | sh"* ]]; then
  echo "error: a browser request did not return the frontpage" >&2
  exit 1
fi

uninstall="$(curl -fsSL --max-time 15 -A "curl/8.0.0" "https://${DOMAIN}/uninstall")"
if [[ "${uninstall}" != "#!/bin/sh"* ]]; then
  echo "error: https://${DOMAIN}/uninstall did not return the uninstall script" >&2
  exit 1
fi

verify_artifact() {
  local artifact="$1"
  local check
  check="$(mktemp -d)"
  if ! curl --proto '=https' --tlsv1.2 -fsSL "https://${DOMAIN}/dl/${artifact}" -o "${check}/${artifact}"; then
    echo "error: https://${DOMAIN}/dl/${artifact} is missing" >&2
    rm -rf "${check}"
    exit 1
  fi
  if ! curl --proto '=https' --tlsv1.2 -fsSL "https://${DOMAIN}/dl/${artifact}.sha256" -o "${check}/${artifact}.sha256"; then
    echo "error: https://${DOMAIN}/dl/${artifact}.sha256 is missing" >&2
    rm -rf "${check}"
    exit 1
  fi
  if ! (cd "${check}" && sha256sum -c "${artifact}.sha256"); then
    echo "error: checksum failed for ${artifact}" >&2
    rm -rf "${check}"
    exit 1
  fi
  rm -rf "${check}"
  echo "Verified ${artifact}."
}

verify_artifact "runebook-linux-x86_64"
verify_artifact "runebook-macos-arm64"
verify_artifact "runebook-macos-x86_64"

echo ""
echo "Install host is live."
echo "  curl -fsSL https://${DOMAIN} | sh"
echo "  curl -fsSL https://${DOMAIN}/uninstall | sh"
