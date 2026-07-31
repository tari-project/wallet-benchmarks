#!/usr/bin/env bash

RUST_TOOLCHAIN="1.96.0"

MINOTARI_REPO="https://github.com/tari-project/minotari-cli.git"
MINOTARI_BASE_REV="360c4848a54d65fd710266233cc9277b0f785e74"
MINOTARI_BASE_TREE="e9bbd1fb7b538e213e17c2986b85940435adce26"
MINOTARI_RESULT_TREE="8ffaba109f7d413a68852a98939ec2a002105cc6"
MINOTARI_COMPLETE_DIFF_SHA256="08b5ff524839091db7c110708fac4538261b94e5c3cdda6961ecdbc42c9c0bcf"
MINOTARI_PASSWORD_PATCH_SHA256="53c7e54c2ef211547196a6cbcc0adb0ce6715390da5188afc3a0150888b9ca7b"

TARI_REPO="https://github.com/tari-project/tari.git"
TARI_CONSOLE_WALLET_REV="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_CONSOLE_WALLET_COMMIT="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_CONSOLE_WALLET_TREE="be2020d2eb904507fa20442448ef76b6e8f0d502"
TARI_NODE_REV="v5.5.0"
TARI_NODE_COMMIT="b32dba96aa0d4ffdd715a0bb724f69c567d47d9f"
TARI_NODE_TREE="449b76bc4121400265927561a661c5c381d55bcd"

PP_REPO="https://github.com/tari-project/minotari_payment_processor.git"
PP_REV="f0572c98cbfac7377412dc6d4094c7d7dfc5de2c"
PP_BASE_TREE="add06a544f950f724caa13b972cfc13e5d666c90"
PP_PATCH_SHA256="69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe"
PP_RESULT_TREE="8f15669442f3da67fc4636de00b80c666d890c5c"
PP_COMPLETE_DIFF_SHA256="8b467bf65003de81ea752092ea3b4f2914e28b284590425d155fda4ad13287d8"

EMPTY_DIFF_SHA256="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

write_build_manifest() {
  local manifest="$1"
  local minotari_sha="$2"
  local console_sha="$3"
  local node_sha="$4"
  local pp_sha="$5"
  cat > "${manifest}" <<EOF
{
  "schema_version": 2,
  "sources": {
    "minotari_cli": {
      "repository": "${MINOTARI_REPO}",
      "upstream": {"revision": "${MINOTARI_BASE_REV}", "commit": "${MINOTARI_BASE_REV}", "tree": "${MINOTARI_BASE_TREE}"},
      "patches": [
        {"path": "patches/minotari-wallet-password-env.patch", "sha256": "${MINOTARI_PASSWORD_PATCH_SHA256}", "result_tree": "${MINOTARI_RESULT_TREE}"}
      ],
      "complete_diff_sha256": "${MINOTARI_COMPLETE_DIFF_SHA256}",
      "result_tree": "${MINOTARI_RESULT_TREE}"
    },
    "tari_console_wallet": {
      "repository": "${TARI_REPO}",
      "upstream": {"revision": "${TARI_CONSOLE_WALLET_REV}", "commit": "${TARI_CONSOLE_WALLET_COMMIT}", "tree": "${TARI_CONSOLE_WALLET_TREE}"},
      "patches": [],
      "complete_diff_sha256": "${EMPTY_DIFF_SHA256}",
      "result_tree": "${TARI_CONSOLE_WALLET_TREE}"
    },
    "minotari_node": {
      "repository": "${TARI_REPO}",
      "upstream": {"revision": "${TARI_NODE_REV}", "commit": "${TARI_NODE_COMMIT}", "tree": "${TARI_NODE_TREE}"},
      "patches": [],
      "complete_diff_sha256": "${EMPTY_DIFF_SHA256}",
      "result_tree": "${TARI_NODE_TREE}"
    },
    "payment_processor": {
      "repository": "${PP_REPO}",
      "upstream": {"revision": "${PP_REV}", "commit": "${PP_REV}", "tree": "${PP_BASE_TREE}"},
      "patches": [
        {"path": "patches/payment-processor-fee-rate.patch", "sha256": "${PP_PATCH_SHA256}", "result_tree": "${PP_RESULT_TREE}"}
      ],
      "complete_diff_sha256": "${PP_COMPLETE_DIFF_SHA256}",
      "result_tree": "${PP_RESULT_TREE}"
    }
  },
  "artifacts": {
    "minotari": {"source": "minotari_cli", "source_revision": "${MINOTARI_BASE_REV}", "source_tree": "${MINOTARI_RESULT_TREE}", "sha256": "${minotari_sha}"},
    "minotari_console_wallet": {"source": "tari_console_wallet", "source_revision": "${TARI_CONSOLE_WALLET_REV}", "source_tree": "${TARI_CONSOLE_WALLET_TREE}", "sha256": "${console_sha}"},
    "minotari_node": {"source": "minotari_node", "source_revision": "${TARI_NODE_REV}", "source_tree": "${TARI_NODE_TREE}", "sha256": "${node_sha}"},
    "minotari_payment_processor": {"source": "payment_processor", "source_revision": "${PP_REV}", "source_tree": "${PP_RESULT_TREE}", "sha256": "${pp_sha}"}
  }
}
EOF
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  if [[ "$#" -ne 6 || "$1" != "--emit-manifest" ]]; then
    printf 'usage: %s --emit-manifest OUT MINOTARI_SHA CONSOLE_SHA NODE_SHA PP_SHA\n' "$0" >&2
    exit 2
  fi
  write_build_manifest "$2" "$3" "$4" "$5" "$6"
fi
