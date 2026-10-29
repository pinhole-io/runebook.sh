#!/bin/sh
# Remove the Runebook binary, the desktop icon, and the user config.
# The vault folder stays on disk.
set -eu

if [ "$(id -u)" -eq 0 ]; then
  echo "error: run the uninstall as your user" >&2
  exit 1
fi

if [ -z "${HOME:-}" ]; then
  echo "error: HOME is empty" >&2
  exit 1
fi

data="${XDG_DATA_HOME:-${HOME}/.local/share}/runebook"
config="${XDG_CONFIG_HOME:-${HOME}/.config}/runebook"
binary="${HOME}/.local/bin/runebook"
desktop="${XDG_DATA_HOME:-${HOME}/.local/share}/applications/runebook.desktop"

remove_tree() {
  dir="$1"
  case "${dir}" in
    ""|"/"|"${HOME}"|"${HOME}/")
      echo "error: refusing to remove ${dir}" >&2
      exit 1
      ;;
  esac
  case "${dir}" in
    */runebook) ;;
    *)
      echo "error: refusing to remove ${dir}" >&2
      exit 1
      ;;
  esac
  if [ -L "${dir}" ]; then
    echo "error: ${dir} is a symlink" >&2
    exit 1
  fi
  if [ -d "${dir}" ]; then
    rm -rf "${dir}"
    echo "Removed ${dir}"
  elif [ -e "${dir}" ]; then
    echo "error: ${dir} is not a directory" >&2
    exit 1
  else
    echo "Already gone: ${dir}"
  fi
}

case "${binary}" in
  "${HOME}/.local/bin/runebook") ;;
  *)
    echo "error: refusing to remove ${binary}" >&2
    exit 1
    ;;
esac

if [ -L "${binary}" ] || [ -f "${binary}" ]; then
  rm -f "${binary}"
  echo "Removed ${binary}"
elif [ -e "${binary}" ]; then
  echo "error: ${binary} is not a file" >&2
  exit 1
else
  echo "Already gone: ${binary}"
fi

case "${desktop}" in
  */applications/runebook.desktop) ;;
  *)
    echo "error: refusing to remove ${desktop}" >&2
    exit 1
    ;;
esac

if [ -L "${desktop}" ]; then
  echo "error: ${desktop} is a symlink" >&2
  exit 1
fi
if [ -f "${desktop}" ]; then
  rm -f "${desktop}"
  echo "Removed ${desktop}"
elif [ -e "${desktop}" ]; then
  echo "error: ${desktop} is not a file" >&2
  exit 1
else
  echo "Already gone: ${desktop}"
fi

remove_tree "${data}"
remove_tree "${config}"

echo "The vault folder was left in place."
