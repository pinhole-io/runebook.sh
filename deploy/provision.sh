#!/usr/bin/env bash
# Launch a small Ubuntu host that serves the Runebook install script.
set -euo pipefail

export AWS_PROFILE="${AWS_PROFILE:-mine}"

REGION="${AWS_REGION:-us-west-2}"
INSTANCE_TYPE="${RUNEBOOK_EC2_INSTANCE_TYPE:-t4g.nano}"
VOLUME_SIZE="${RUNEBOOK_EC2_VOLUME_GB:-8}"
KEY_NAME="${RUNEBOOK_EC2_KEY_NAME:-runebook-install}"
SG_NAME="${RUNEBOOK_EC2_SG_NAME:-runebook-install-sg}"
INSTANCE_NAME="${RUNEBOOK_EC2_INSTANCE_NAME:-runebook-install}"
DOMAIN="${RUNEBOOK_DOMAIN:-runbook.sh}"
PUB_KEY="${RUNEBOOK_EC2_PUBLIC_KEY:-${HOME}/.ssh/default.pub}"
STATE_FILE="${HOME}/.runebook/install-host.env"

mkdir -p "${HOME}/.runebook"

write_state() {
  local instance_id="$1"
  local public_ip="$2"
  local alloc_id="$3"
  cat > "${STATE_FILE}" <<EOF
RUNEBOOK_EC2_INSTANCE_ID=${instance_id}
RUNEBOOK_EC2_PUBLIC_IP=${public_ip}
RUNEBOOK_EC2_EIP_ALLOC_ID=${alloc_id}
RUNEBOOK_EC2_REGION=${REGION}
RUNEBOOK_EC2_KEY_NAME=${KEY_NAME}
RUNEBOOK_EC2_INSTANCE_NAME=${INSTANCE_NAME}
RUNEBOOK_EC2_USER=ubuntu
RUNEBOOK_DOMAIN=${DOMAIN}
EOF
  chmod 600 "${STATE_FILE}"
}

ensure_eip() {
  local instance_id="$1"
  local alloc_id="" public_ip="" associated="" err_file
  err_file="$(mktemp)"

  alloc_id="$(aws ec2 describe-addresses \
    --region "${REGION}" \
    --filters "Name=tag:Name,Values=${INSTANCE_NAME}" \
    --query 'Addresses[0].AllocationId' \
    --output text)"
  if [[ -n "${alloc_id}" && "${alloc_id}" != "None" ]]; then
    public_ip="$(aws ec2 describe-addresses \
      --region "${REGION}" \
      --allocation-ids "${alloc_id}" \
      --query 'Addresses[0].PublicIp' \
      --output text)"
    associated="$(aws ec2 describe-addresses \
      --region "${REGION}" \
      --allocation-ids "${alloc_id}" \
      --query 'Addresses[0].InstanceId' \
      --output text)"
    if [[ "${associated}" != "${instance_id}" ]]; then
      aws ec2 associate-address \
        --instance-id "${instance_id}" \
        --allocation-id "${alloc_id}" \
        --region "${REGION}" >/dev/null
    fi
    write_state "${instance_id}" "${public_ip}" "${alloc_id}"
    rm -f "${err_file}"
    echo "${public_ip}"
    return 0
  fi

  echo "Allocating an Elastic IP..." >&2
  if alloc_id="$(aws ec2 allocate-address \
    --domain vpc \
    --region "${REGION}" \
    --tag-specifications "ResourceType=elastic-ip,Tags=[{Key=Name,Value=${INSTANCE_NAME}}]" \
    --query 'AllocationId' \
    --output text 2>"${err_file}")"; then
    public_ip="$(aws ec2 describe-addresses \
      --region "${REGION}" \
      --allocation-ids "${alloc_id}" \
      --query 'Addresses[0].PublicIp' \
      --output text)"
    aws ec2 associate-address \
      --instance-id "${instance_id}" \
      --allocation-id "${alloc_id}" \
      --region "${REGION}" >/dev/null
    write_state "${instance_id}" "${public_ip}" "${alloc_id}"
    rm -f "${err_file}"
    echo "${public_ip}"
    return 0
  fi

  if grep -q "AddressLimitExceeded" "${err_file}"; then
    public_ip="$(aws ec2 describe-instances \
      --region "${REGION}" \
      --instance-ids "${instance_id}" \
      --query 'Reservations[0].Instances[0].PublicIpAddress' \
      --output text)"
    rm -f "${err_file}"
    if [[ -z "${public_ip}" || "${public_ip}" == "None" ]]; then
      echo "error: Elastic IP quota is full and the instance has no public IP" >&2
      return 1
    fi
    echo "Elastic IP quota is full. Using the instance public IP." >&2
    echo "This address changes if you stop the instance." >&2
    write_state "${instance_id}" "${public_ip}" ""
    echo "${public_ip}"
    return 0
  fi

  cat "${err_file}" >&2
  rm -f "${err_file}"
  return 1
}

if [[ -f "${STATE_FILE}" ]]; then
  # shellcheck disable=SC1090
  source "${STATE_FILE}"
  REGION="${RUNEBOOK_EC2_REGION:-${REGION}}"
  DOMAIN="${RUNEBOOK_DOMAIN:-${DOMAIN}}"
  INSTANCE_NAME="${RUNEBOOK_EC2_INSTANCE_NAME:-${INSTANCE_NAME}}"
  instance_state="$(aws ec2 describe-instances \
    --region "${REGION}" \
    --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}" \
    --query 'Reservations[0].Instances[0].State.Name' \
    --output text 2>/dev/null || true)"
  case "${instance_state}" in
    pending)
      aws ec2 wait instance-running --region "${REGION}" --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}"
      ;;
    stopping)
      aws ec2 wait instance-stopped --region "${REGION}" --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}"
      aws ec2 start-instances --region "${REGION}" --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}" >/dev/null
      aws ec2 wait instance-running --region "${REGION}" --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}"
      ;;
    stopped)
      aws ec2 start-instances --region "${REGION}" --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}" >/dev/null
      aws ec2 wait instance-running --region "${REGION}" --instance-ids "${RUNEBOOK_EC2_INSTANCE_ID}"
      ;;
    running) ;;
    "")
      echo "error: cannot read instance ${RUNEBOOK_EC2_INSTANCE_ID}" >&2
      echo "Fix AWS credentials, or remove ${STATE_FILE} if that instance is gone." >&2
      exit 1
      ;;
    *)
      echo "error: instance ${RUNEBOOK_EC2_INSTANCE_ID} is ${instance_state}." >&2
      echo "Remove ${STATE_FILE} before you create a new host." >&2
      exit 1
      ;;
  esac
  if ! PUBLIC_IP="$(ensure_eip "${RUNEBOOK_EC2_INSTANCE_ID}")"; then
    exit 1
  fi
  echo "Using instance ${RUNEBOOK_EC2_INSTANCE_ID}."
  echo "Elastic IP: ${PUBLIC_IP}"
  echo "State file: ${STATE_FILE}"
  echo "Registrar record: ${DOMAIN}  A  ${PUBLIC_IP}"
  exit 0
fi

CALLER_ARN="$(aws sts get-caller-identity --region "${REGION}" --query Arn --output text 2>/dev/null || true)"
if [[ -z "${CALLER_ARN}" || "${CALLER_ARN}" == "None" ]]; then
  echo "error: AWS credentials not configured for profile ${AWS_PROFILE}" >&2
  echo "Log in first: aws login --profile ${AWS_PROFILE}" >&2
  exit 1
fi

if [[ ! -f "${PUB_KEY}" ]]; then
  echo "error: SSH public key not found at ${PUB_KEY}" >&2
  exit 1
fi

MY_IP="$(curl -fsS https://ifconfig.me || curl -fsS https://icanhazip.com || true)"
if [[ ! "${MY_IP}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: could not determine this machine's public IPv4" >&2
  exit 1
fi

echo "Region:  ${REGION}"
echo "Profile: ${AWS_PROFILE}"
echo "Caller:  ${CALLER_ARN}"
echo "Domain:  ${DOMAIN}"
echo "SSH:     ${MY_IP}/32"

ARCH="amd64"
case "${INSTANCE_TYPE}" in
  t4g.*|c6g.*|c7g.*|m6g.*|m7g.*) ARCH="arm64" ;;
esac

AMI_ID="$(aws ssm get-parameter \
  --region "${REGION}" \
  --name "/aws/service/canonical/ubuntu/server/24.04/stable/current/${ARCH}/hvm/ebs-gp3/ami-id" \
  --query 'Parameter.Value' \
  --output text)"

if ! aws ec2 describe-key-pairs --key-names "${KEY_NAME}" --region "${REGION}" >/dev/null 2>&1; then
  aws ec2 import-key-pair \
    --key-name "${KEY_NAME}" \
    --public-key-material "fileb://${PUB_KEY}" \
    --region "${REGION}" >/dev/null
fi

VPC_ID="$(aws ec2 describe-vpcs \
  --region "${REGION}" \
  --filters Name=isDefault,Values=true \
  --query 'Vpcs[0].VpcId' \
  --output text)"
if [[ -z "${VPC_ID}" || "${VPC_ID}" == "None" ]]; then
  echo "error: no default VPC in ${REGION}" >&2
  exit 1
fi

SG_ID="$(aws ec2 describe-security-groups \
  --region "${REGION}" \
  --filters "Name=group-name,Values=${SG_NAME}" "Name=vpc-id,Values=${VPC_ID}" \
  --query 'SecurityGroups[0].GroupId' \
  --output text 2>/dev/null || true)"
if [[ -z "${SG_ID}" || "${SG_ID}" == "None" ]]; then
  SG_ID="$(aws ec2 create-security-group \
    --region "${REGION}" \
    --group-name "${SG_NAME}" \
    --description "Runebook install host" \
    --vpc-id "${VPC_ID}" \
    --query 'GroupId' \
    --output text)"
fi

allow_port() {
  local port="$1"
  local cidr="$2"
  local err=""
  if ! err="$(aws ec2 authorize-security-group-ingress \
    --region "${REGION}" \
    --group-id "${SG_ID}" \
    --protocol tcp --port "${port}" --cidr "${cidr}" 2>&1 >/dev/null)"; then
    if [[ "${err}" != *"InvalidPermission.Duplicate"* ]]; then
      echo "${err}" >&2
      exit 1
    fi
  fi
}

allow_port 22 "${MY_IP}/32"
allow_port 80 "0.0.0.0/0"
allow_port 443 "0.0.0.0/0"

echo "Launching ${INSTANCE_TYPE} (${AMI_ID})..."
INSTANCE_ID="$(aws ec2 run-instances \
  --region "${REGION}" \
  --image-id "${AMI_ID}" \
  --instance-type "${INSTANCE_TYPE}" \
  --key-name "${KEY_NAME}" \
  --security-group-ids "${SG_ID}" \
  --metadata-options "HttpTokens=required,HttpEndpoint=enabled,HttpPutResponseHopLimit=2" \
  --block-device-mappings "[{\"DeviceName\":\"/dev/sda1\",\"Ebs\":{\"VolumeSize\":${VOLUME_SIZE},\"VolumeType\":\"gp3\",\"DeleteOnTermination\":true}}]" \
  --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=${INSTANCE_NAME}}]" \
  --query 'Instances[0].InstanceId' \
  --output text)"

aws ec2 wait instance-running --region "${REGION}" --instance-ids "${INSTANCE_ID}"
write_state "${INSTANCE_ID}" "" ""
if ! PUBLIC_IP="$(ensure_eip "${INSTANCE_ID}")"; then
  exit 1
fi

echo ""
echo "Host is up."
echo "  Instance: ${INSTANCE_ID}"
echo "  Elastic IP: ${PUBLIC_IP}"
echo ""
echo "At the registrar, set this record and turn off the parking page:"
echo "  ${DOMAIN}  A  ${PUBLIC_IP}"
echo ""
echo "Next: ./deploy/setup-server.sh"
