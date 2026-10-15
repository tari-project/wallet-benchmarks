#!/usr/bin/env bash
#
# Builds every binary the harness spawns, each at its pinned revision.
#
# The harness asserts at start-up that each of these reports the version in the `[pins]`
# section of benchmark.toml and refuses to run otherwise, so a profile always names the
# software it was produced by. Nothing here is committed: binaries are built, and `/tools/*`
# is git-ignored apart from this script.
#
#   ./tools/fetch.sh            # build everything
#   ./tools/fetch.sh minotari   # build one target
#
# Targets: console_wallet, minotari, payment_processor, offline_signer
#
# Requires: git, cargo (Rust >= 1.85 — the payment processor is edition 2024), protoc
# (the console wallet's gRPC is generated at build time), and a C toolchain.

set -euo pipefail

TOOLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="${TOOLS_DIR}/src"

# --- pinned revisions -------------------------------------------------------------------
#
# Keep these in step with the `[pins]` versions in benchmark.example.toml. Changing one
# without the other makes the start-up assertion fail, which is the point.

# tari, for `minotari_console_wallet` (mode 1). Reports `minotari_console_wallet 6.2.0-pre.1`.
TARI_REPO="https://github.com/tari-project/tari.git"
TARI_REV="v6.2.0-pre.1"

# tari again, at a *different* revision, for `minotari_offline_signer` (mode 3).
#
# This is not a mistake and it must not be "tidied up" to one checkout. The payment processor
# pins `tari_transaction_components` at 766f80cc, which speaks offline-signing payload format
# 4.0.0. tari v5.4.0 moved that format to 5.0.0 and added a mandatory `payload_signature`
# field, so a signer built from v5.4.0 or later can neither read the payloads the processor
# produces nor produce output it can parse — every signing attempt fails and every batch
# exhausts its retries. 9f5adb7 (v5.3.0-pre.3) is the earliest tari release that ships the
# signer while still on 4.0.0.
SIGNER_REV="9f5adb7"

# minotari-cli, for the `minotari` daemon (mode 3's payment receiver). Reports `tari 0.3.0`:
# the executable is `minotari` but its clap command is named `tari`. This is the same
# revision the harness links as a library; keep it in step with `Cargo.toml` and
# `metrics::MINOTARI_CLI_REV`.
MINOTARI_CLI_REPO="https://github.com/tari-project/minotari-cli.git"
MINOTARI_CLI_REV="1ab7c8c651b2937a8b05fba9d5a432cf633ae3f8"

# minotari_payment_processor (mode 3). Takes no `--version` flag; the harness reads its
# version from `GET /health/version` once it is up.
PP_REPO="https://github.com/tari-project/minotari_payment_processor.git"
PP_REV="4c5d3cfa965a065d4d5f81a8db672286cacdb2bd"

# ----------------------------------------------------------------------------------------

log() { printf '\n== %s\n' "$*" >&2; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: $1 is required but not on PATH" >&2
        exit 1
    }
}

# Clones `repo` into `dir` if missing, then checks out exactly `rev`.
checkout() {
    local repo="$1" dir="$2" rev="$3"
    if [ ! -d "${dir}/.git" ]; then
        log "cloning ${repo} -> ${dir}"
        mkdir -p "$(dirname "${dir}")"
        git clone --no-checkout "${repo}" "${dir}"
    fi
    log "checking out ${rev} in ${dir}"
    git -C "${dir}" fetch --tags --quiet origin
    git -C "${dir}" checkout --quiet --force "${rev}"
}

# Builds one cargo package and copies the resulting binary into tools/.
build() {
    local dir="$1" package="$2" binary="$3"
    log "building ${package} (${dir})"
    (cd "${dir}" && cargo build --release --locked -p "${package}")
    cp "${dir}/target/release/${binary}" "${TOOLS_DIR}/${binary}"
    log "${TOOLS_DIR}/${binary}"
}

build_console_wallet() {
    checkout "${TARI_REPO}" "${SRC_DIR}/tari" "${TARI_REV}"
    build "${SRC_DIR}/tari" minotari_console_wallet minotari_console_wallet
    "${TOOLS_DIR}/minotari_console_wallet" --version
}

build_offline_signer() {
    # A second checkout on purpose — see SIGNER_REV above.
    checkout "${TARI_REPO}" "${SRC_DIR}/tari-signer" "${SIGNER_REV}"
    build "${SRC_DIR}/tari-signer" minotari_offline_signer minotari_offline_signer
    "${TOOLS_DIR}/minotari_offline_signer" --version
}

build_minotari() {
    checkout "${MINOTARI_CLI_REPO}" "${SRC_DIR}/minotari-cli" "${MINOTARI_CLI_REV}"
    build "${SRC_DIR}/minotari-cli" minotari minotari
    "${TOOLS_DIR}/minotari" --version
}

build_payment_processor() {
    checkout "${PP_REPO}" "${SRC_DIR}/minotari_payment_processor" "${PP_REV}"
    build "${SRC_DIR}/minotari_payment_processor" minotari_payment_processor \
        minotari_payment_processor
    # No `--version` flag exists on this binary; the harness reads the version from
    # `GET /health/version` after it spawns the process.
    echo "built (this binary takes no arguments; version is read over HTTP at runtime)" >&2
}

require git
require cargo

targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
    targets=(console_wallet minotari payment_processor offline_signer)
fi

for target in "${targets[@]}"; do
    case "${target}" in
        console_wallet) build_console_wallet ;;
        minotari) build_minotari ;;
        payment_processor) build_payment_processor ;;
        offline_signer) build_offline_signer ;;
        *)
            echo "error: unknown target '${target}'." >&2
            echo "       known targets: console_wallet minotari payment_processor offline_signer" >&2
            exit 1
            ;;
    esac
done

log "done. Point benchmark.toml at ${TOOLS_DIR}/ and run \`cargo run --release -- plan\`."
