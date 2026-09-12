#!/usr/bin/env bash
# Fetch the reviewed installer; it selects and verifies the latest testing build.
nuncio_install_testing() (
  set -euo pipefail
  umask 077

  installer_commit='13dbe549cb386fe641ef82922916fafe7cc9cf5c'
  installer_sha256='d786aa95d485bcc13cbb627ccd761bc3621109a04263bdc5c05cd9d02ccd0aff'

  for prerequisite in python3 gh curl shasum; do
    if ! command -v "$prerequisite" >/dev/null 2>&1; then
      printf 'Nuncio installer needs %s. Install Python 3.11+ and GitHub CLI 2.100+, then run gh auth login --hostname github.com.\n' "$prerequisite" >&2
      exit 1
    fi
  done
  if ! python3 -I -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' 2>/dev/null; then
    printf 'Nuncio installer needs Python 3.11 or newer available as python3.\n' >&2
    exit 1
  fi

  nuncio_bootstrap_tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/nuncio-bootstrap.XXXXXXXX")
  trap 'rm -rf -- "$nuncio_bootstrap_tmpdir"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  installer_file="$nuncio_bootstrap_tmpdir/install-testing.py"
  installer_url="https://raw.githubusercontent.com/KofTwentyTwo/nuncio/$installer_commit/rebuild/scripts/install-testing.py"
  if ! curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --connect-timeout 15 --max-time 120 --retry 2 --max-filesize 1048576 \
    --output "$installer_file" "$installer_url"; then
    printf 'Nuncio installer download failed; nothing was installed.\n' >&2
    exit 1
  fi
  actual_sha256=$(shasum -a 256 "$installer_file")
  if [[ "${actual_sha256%% *}" != "$installer_sha256" ]]; then
    printf 'Nuncio installer checksum mismatch; nothing was executed or installed.\n' >&2
    exit 1
  fi

  python3 -I "$installer_file" "$@" </dev/null
)

nuncio_install_testing "$@"
