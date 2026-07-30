#!/usr/bin/env bash
set -euo pipefail

VERIFY_ONLY=false
POSITIONAL=()
for arg in "$@"; do
  case "${arg}" in
    --verify-only) VERIFY_ONLY=true ;;
    *) POSITIONAL+=("${arg}") ;;
  esac
done
if [ "${#POSITIONAL[@]}" -gt 2 ]; then
  printf 'usage: %s [--verify-only] [CACHE_DIR] [TOOLS_DIR]\n' "$0" >&2
  exit 2
fi

CACHE_DIR="${POSITIONAL[0]:-.bench-cache}"
TOOLS_DIR="${POSITIONAL[1]:-tools}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCH_DIR="${SCRIPT_DIR}/../patches"
source "${SCRIPT_DIR}/build-provenance.sh"
MINOTARI_DIR="${CACHE_DIR}/minotari-cli"

TARI_DIR="${CACHE_DIR}/tari"

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    sha256sum "$1" | cut -d ' ' -f 1
  fi
}

sha256_stdin() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | cut -d ' ' -f 1
  else
    sha256sum | cut -d ' ' -f 1
  fi
}

require_sha256() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(sha256_file "${path}")"
  if [ "${actual}" != "${expected}" ]; then
    printf 'SHA-256 mismatch for %s: expected %s, got %s\n' "${path}" "${expected}" "${actual}" >&2
    exit 1
  fi
}

require_clean_repository() {
  local directory="$1"
  local label="$2"
  if [ -n "$(git -C "${directory}" status --porcelain --untracked-files=all)" ]; then
    printf '%s source tree is dirty; use a fresh cache directory\n' "${label}" >&2
    exit 1
  fi
}

verify_checkout() {
  local directory="$1"
  local revision="$2"
  local expected_commit="$3"
  local expected_tree="$4"
  local label="$5"
  git -C "${directory}" checkout --detach "${revision}"
  if [ "$(git -C "${directory}" rev-parse HEAD)" != "${expected_commit}" ]; then
    printf '%s revision did not resolve to expected commit %s\n' "${label}" "${expected_commit}" >&2
    exit 1
  fi
  if [ "$(git -C "${directory}" rev-parse HEAD^{tree})" != "${expected_tree}" ]; then
    printf '%s upstream tree does not match expected tree %s\n' "${label}" "${expected_tree}" >&2
    exit 1
  fi
  require_clean_repository "${directory}" "${label}"
}

apply_minotari_patch() {
  local file_name="$1"
  local expected_sha="$2"
  local expected_tree="$3"
  local patch_path="${PATCH_DIR}/${file_name}"
  require_sha256 "${patch_path}" "${expected_sha}"
  git -C "${MINOTARI_DIR}" apply --check --index "${patch_path}"
  git -C "${MINOTARI_DIR}" apply --index "${patch_path}"
  local actual_tree
  actual_tree="$(git -C "${MINOTARI_DIR}" write-tree)"
  if [ "${actual_tree}" != "${expected_tree}" ]; then
    printf 'Minotari result tree after %s does not match expected tree %s\n' "${file_name}" "${expected_tree}" >&2
    exit 1
  fi
}

mkdir -p "${CACHE_DIR}"

if [ ! -d "${MINOTARI_DIR}/.git" ]; then
  git clone "${MINOTARI_REPO}" "${MINOTARI_DIR}"
fi
require_clean_repository "${MINOTARI_DIR}" "minotari-cli"
git -C "${MINOTARI_DIR}" remote set-url origin "${MINOTARI_REPO}"
git -C "${MINOTARI_DIR}" fetch --tags origin
verify_checkout "${MINOTARI_DIR}" "${MINOTARI_BASE_REV}" "${MINOTARI_BASE_REV}" "${MINOTARI_BASE_TREE}" "minotari-cli"

cleanup_minotari() {
  git -C "${MINOTARI_DIR}" reset --hard "${MINOTARI_BASE_REV}" >/dev/null 2>&1 || true
}
trap cleanup_minotari EXIT

apply_minotari_patch \
  "minotari-wallet-password-env.patch" \
  "${MINOTARI_PASSWORD_PATCH_SHA256}" \
  "${MINOTARI_RESULT_TREE}"

git -C "${MINOTARI_DIR}" diff --cached --check
if ! git -C "${MINOTARI_DIR}" diff --quiet; then
  printf 'Minotari source contains unstaged changes beyond the tracked patches\n' >&2
  exit 1
fi
if [ -n "$(git -C "${MINOTARI_DIR}" ls-files --others --exclude-standard)" ]; then
  printf 'Minotari source contains untracked files beyond the tracked patches\n' >&2
  exit 1
fi
ACTUAL_COMPLETE_DIFF_SHA256="$(git -c diff.algorithm=myers -C "${MINOTARI_DIR}" diff --cached --full-index --binary --no-ext-diff --no-textconv --no-renames "${MINOTARI_BASE_REV}" | sha256_stdin)"
if [ "${ACTUAL_COMPLETE_DIFF_SHA256}" != "${MINOTARI_COMPLETE_DIFF_SHA256}" ]; then
  printf 'Minotari complete diff SHA-256 mismatch: expected %s, got %s\n' \
    "${MINOTARI_COMPLETE_DIFF_SHA256}" "${ACTUAL_COMPLETE_DIFF_SHA256}" >&2
  exit 1
fi

if [ ! -d "${TARI_DIR}/.git" ]; then
  git clone "${TARI_REPO}" "${TARI_DIR}"
fi
require_clean_repository "${TARI_DIR}" "Tari"
git -C "${TARI_DIR}" remote set-url origin "${TARI_REPO}"
git -C "${TARI_DIR}" fetch --tags origin
verify_checkout \
  "${TARI_DIR}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_CONSOLE_WALLET_COMMIT}" \
  "${TARI_CONSOLE_WALLET_TREE}" "Tari console wallet"

if [ "${VERIFY_ONLY}" = true ]; then
  verify_checkout "${TARI_DIR}" "${TARI_NODE_REV}" "${TARI_NODE_COMMIT}" "${TARI_NODE_TREE}" "Tari node"
  printf 'source provenance PASS: minotari base %s + runtime-only password patch -> %s; console %s; node %s\n' \
    "${MINOTARI_BASE_REV}" "${MINOTARI_RESULT_TREE}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_NODE_REV}"
  exit 0
fi

(
  cd "${MINOTARI_DIR}"
  env TARI_TARGET_NETWORK=testnet cargo +"${RUST_TOOLCHAIN}" build --release --locked --bin minotari
)

(
  cd "${TARI_DIR}"
  env TARI_TARGET_NETWORK=testnet cargo +"${RUST_TOOLCHAIN}" build --release --locked --bin minotari_console_wallet
)

verify_checkout "${TARI_DIR}" "${TARI_NODE_REV}" "${TARI_NODE_COMMIT}" "${TARI_NODE_TREE}" "Tari node"
(
  cd "${TARI_DIR}"
  env TARI_TARGET_NETWORK=testnet cargo +"${RUST_TOOLCHAIN}" build --release --locked --bin minotari_node
)

mkdir -p "${TOOLS_DIR}"
cp "${MINOTARI_DIR}/target/release/minotari" "${TOOLS_DIR}/minotari"
cp "${TARI_DIR}/target/release/minotari_console_wallet" "${TOOLS_DIR}/minotari_console_wallet"
cp "${TARI_DIR}/target/release/minotari_node" "${TOOLS_DIR}/minotari_node"

printf 'installed minotari from upstream %s with runtime-only password patch, minotari_console_wallet at %s, and minotari_node at %s in %s\n' \
  "${MINOTARI_BASE_REV}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_NODE_REV}" "${TOOLS_DIR}"
