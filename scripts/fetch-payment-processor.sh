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
PATCH_FILE="${SCRIPT_DIR}/../patches/payment-processor-fee-rate.patch"
MANIFEST="${TOOLS_DIR}/build-manifest.json"
PROTOC="${SCRIPT_DIR}/protoc-compat.sh"
source "${SCRIPT_DIR}/build-provenance.sh"
PP_DIR="${CACHE_DIR}/minotari_payment_processor"

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

normalize_macos_signature() {
  if [ "$(uname -s)" = "Darwin" ]; then
    codesign --force --sign - "$1"
    codesign --verify --strict "$1"
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
  if [ -n "$(git -C "$1" status --porcelain --untracked-files=all)" ]; then
    printf '%s source tree is dirty; use a fresh cache directory\n' "$2" >&2
    exit 1
  fi
}

"${SCRIPT_DIR}/fetch-minotari-cli.sh" --verify-only "${CACHE_DIR}" "${TOOLS_DIR}"

mkdir -p "${CACHE_DIR}"
if [ ! -d "${PP_DIR}/.git" ]; then
  git clone "${PP_REPO}" "${PP_DIR}"
fi
require_clean_repository "${PP_DIR}" "payment-processor"
git -C "${PP_DIR}" remote set-url origin "${PP_REPO}"
git -C "${PP_DIR}" fetch --tags origin
git -C "${PP_DIR}" checkout --detach "${PP_REV}"
if [ "$(git -C "${PP_DIR}" rev-parse HEAD)" != "${PP_REV}" ] ||
   [ "$(git -C "${PP_DIR}" rev-parse HEAD^{tree})" != "${PP_BASE_TREE}" ]; then
  printf 'payment-processor upstream revision/tree verification failed\n' >&2
  exit 1
fi
require_clean_repository "${PP_DIR}" "payment-processor"

cleanup_payment_processor() {
  git -C "${PP_DIR}" reset --hard "${PP_REV}" >/dev/null 2>&1 || true
}
trap cleanup_payment_processor EXIT

require_sha256 "${PATCH_FILE}" "${PP_PATCH_SHA256}"
git -C "${PP_DIR}" apply --check --index "${PATCH_FILE}"
git -C "${PP_DIR}" apply --index "${PATCH_FILE}"
if [ "$(git -C "${PP_DIR}" write-tree)" != "${PP_RESULT_TREE}" ]; then
  printf 'payment-processor patched tree does not match expected tree %s\n' "${PP_RESULT_TREE}" >&2
  exit 1
fi
git -C "${PP_DIR}" diff --cached --check
if ! git -C "${PP_DIR}" diff --quiet; then
  printf 'payment-processor source contains unstaged changes beyond the tracked patch\n' >&2
  exit 1
fi
if [ -n "$(git -C "${PP_DIR}" ls-files --others --exclude-standard)" ]; then
  printf 'payment-processor source contains untracked files beyond the tracked patch\n' >&2
  exit 1
fi
ACTUAL_COMPLETE_DIFF_SHA256="$(git -c diff.algorithm=myers -C "${PP_DIR}" diff --cached --full-index --binary --no-ext-diff --no-textconv --no-renames "${PP_REV}" | sha256_stdin)"
if [ "${ACTUAL_COMPLETE_DIFF_SHA256}" != "${PP_COMPLETE_DIFF_SHA256}" ]; then
  printf 'payment-processor complete diff SHA-256 mismatch: expected %s, got %s\n' \
    "${PP_COMPLETE_DIFF_SHA256}" "${ACTUAL_COMPLETE_DIFF_SHA256}" >&2
  exit 1
fi

if [ "${VERIFY_ONLY}" = true ]; then
  printf 'source provenance PASS: payment processor %s + tracked fee patch -> %s\n' \
    "${PP_REV}" "${PP_RESULT_TREE}"
  exit 0
fi

mkdir -p "${PP_DIR}/data"
(
  cd "${PP_DIR}"
  rm -f data/payments.db
  for migration in migrations/*.sql; do
    sqlite3 data/payments.db < "${migration}"
  done
  DATABASE_URL=sqlite://data/payments.db \
    PROTOC="${PROTOC}" \
    TARI_TARGET_NETWORK=testnet \
    cargo +"${RUST_TOOLCHAIN}" build --release --locked
)

for artifact in minotari minotari_console_wallet minotari_node; do
  if [ ! -f "${TOOLS_DIR}/${artifact}" ]; then
    printf 'required artifact %s is missing; run fetch-minotari-cli.sh without --verify-only first\n' \
      "${TOOLS_DIR}/${artifact}" >&2
    exit 1
  fi
done
mkdir -p "${TOOLS_DIR}"
cp "${PP_DIR}/target/release/minotari_payment_processor" "${TOOLS_DIR}/minotari_payment_processor"
normalize_macos_signature "${TOOLS_DIR}/minotari_payment_processor"

MINOTARI_SHA="$(sha256_file "${TOOLS_DIR}/minotari")"
CONSOLE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_console_wallet")"
NODE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_node")"
PP_SHA="$(sha256_file "${TOOLS_DIR}/minotari_payment_processor")"

write_build_manifest "${MANIFEST}" "${MINOTARI_SHA}" "${CONSOLE_SHA}" "${NODE_SHA}" "${PP_SHA}"

printf 'built payment processor at %s, verified exact source provenance, and wrote schema-v2 manifest %s\n' \
  "${PP_REV}" "${MANIFEST}"
