# Tari Wallet Benchmarks

Reproducible Esmeralda benchmark harness for:

- `minotari_console_wallet` over gRPC
- the `minotari` Rust library with offline signing
- `minotari_payment_processor` batch payments

The canonical protocol is `B0,S0,S1,S2,S3,S4,S5,S6,S7`. Wallet rejection,
locking, contention, stalls, and timeouts are measured outcomes. The harness
does not retry scenario transactions, unlock timed-out S4 selections, or
pre-partition UTXOs.

## Prerequisites

- Rustup; `rust-toolchain.toml` installs the pinned Rust toolchain, `rustfmt`, and `clippy`
- Git, Bash, `curl`, `lsof`, `perl`, `sqlite3`, `protobuf-compiler`, and standard C/C++ build tools
- Node.js/npm only for installing `@ast-grep/cli`
- An unpruned, synchronized Esmeralda HTTP wallet-query endpoint
- A public Esmeralda authority endpoint
- A separate funded source wallet DB for the one external S0 funding transaction

macOS:

```sh
xcode-select --install
brew install rustup git protobuf sqlite3 node
npm install --global @ast-grep/cli
```

Ubuntu/Debian:

```sh
sudo apt-get update
sudo apt-get install -y build-essential clang cmake git curl lsof perl protobuf-compiler sqlite3 nodejs npm
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
npm install --global @ast-grep/cli
```

## Fresh Clone

```sh
git clone https://github.com/tari-project/wallet-benchmarks.git
cd wallet-benchmarks
scripts/fetch-minotari-cli.sh .bench-cache tools
scripts/fetch-payment-processor.sh .bench-cache tools
cargo build --release --locked --features live-minotari
cp harness-prefunding.toml harness.toml
```

The repository-level Cargo configuration force-pins `TARI_TARGET_NETWORK` to
`testnet`, so ambient shell settings cannot silently build mainnet or nextnet
artifacts.

Before using the template, follow `RUNBOOK.md` to initialize and synchronize the
pinned local node, then replace `REPLACE_WITH_LOCAL_NODE_PUBLIC_KEY` with its
`whoami` public key. Canonical live configuration rejects a remote scan endpoint,
a local authority endpoint, or identical scan/authority URLs.

The harness links the unmodified upstream Minotari library. The runtime Minotari
CLI patch accepts a hidden environment password and binds its unauthenticated
control API to loopback instead of every interface. The payment processor adds
its required fee-rate patch. Neither patch changes Mode 2 wallet selection or
scanning. The harness writes the payment receiver an explicit, candidate-scoped
Esmeralda configuration instead of allowing ambient Minotari defaults.
The fetchers verify immutable patch SHA-256 values, final Git trees, and complete
diffs before building. The second fetcher writes typed schema-v2
`tools/build-manifest.json`; preflight fails closed unless its upstream bases,
ordered patches, result trees, and every runtime artifact SHA-256 match the
provenance embedded by `build.rs`. CI performs the source checks without building
large binaries via each script's `--verify-only` mode and verifies that the
shared manifest emitter matches the embedded canonical claims. Canonical runs
also record clean-worktree evidence at measurement and export and require one
identical lowercase 40-hex harness commit for both.
See `THIRD_PARTY_NOTICES.md` for the upstream licensing boundary; runtime
binaries and fetched source trees are not redistributed by this repository.

Mode 3 preserves the pinned payment processor's five worker cadences separately:
batch creator `15s`, unsigned transaction creator `15s`, transaction signer
`10s`, broadcaster `15s`, and confirmation checker `60s`. The result profile
records each value and submission validation rejects tuning. The companion
wallet's `10s` scan interval is separate from those worker settings.
Minotari's control API has no upstream authentication; the runtime patch binds
it to loopback, but live runs still require a trusted, single-user host.

For a local node, set `network.base_node_http_url` to its HTTP endpoint and set
`network.mode1_base_node_service_peer` to `public_key::multiaddr`. Keep
`network.authority_http_url` on public Esmeralda. The harness requires an
archival selected endpoint with both `pruning_horizon=0` and `pruned_height=0`,
compares its finalized hash with the authority, and rejects stale local nodes.

## Candidate Workflow

Use a new `paths.data_dir`, matching `modes.new_wallet_database`, and new seed env
file for every candidate. Keep at least 20 GiB free on the candidate volume.

```sh
mkdir -p .secrets candidates
cargo run --release --locked -- addresses \
  --config harness.toml \
  --out .secrets/candidate.env
set -a
. .secrets/candidate.env
set +a
read -r -s HARNESS_WALLET_PW
export HARNESS_WALLET_PW

cargo run --release --locked --features live-minotari -- baseline-workflow \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --s0-evidence candidates/s0-funding.json \
  --profile candidates/esmeralda-baseline.json \
  --summary candidates/esmeralda-baseline.md
```

The workflow performs disk/build-manifest checks once, then runs B0, resumable S0
funding, recipient synchronization/readiness, the benchmark, submission
validation, and summary generation in one process. `fund-s0` still writes a
broadcast checkpoint atomically before waiting for `C_min`; the standalone stage
commands remain available for diagnosis and interrupted-funding recovery.

The operator funds exactly one source wallet, not any benchmark-mode address.
The private `harness.toml` must bind that wallet before any source DB is opened:

```toml
[source_wallet]
database = "/absolute/path/to/source-wallet.db"
address = "THE_ESMERALDA_SOURCE_ADDRESS"
wallet_fingerprint_hex = "THE_HEX_DEFAULT_ACCOUNT_FINGERPRINT"
```

The harness requires `--source-db` to canonicalize to that exact path, verifies
that the address is Esmeralda and hashes to the configured fingerprint, then
checks the database fingerprint read-only. Before any spend or recovery
submission, it also requires the wallet's latest persisted scan hash to match
the same height on both the selected Esmeralda node and the independent
Esmeralda authority. This prevents an unrelated wallet DB such as a Tari
Universe mainnet wallet from being selected accidentally.
After all three empty-wallet B0 scans finish, the harness automatically broadcasts
one transaction containing three `A_fund` outputs, one to each fresh mode seed,
and waits for recipient readiness. An exact-tip scan passes; a library scanner
that returns below the immutable old-wallet anchor is retained as a completed
wallet failure without reinvocation. The source wallet is not measured; its
shared funding fee is disclosed but not deducted from any mode balance.
The generated `recipient_sink` seed starts unfunded and is never opened or
scanned by the harness; S4 and S5 use its recoverable address with distinct
embedded payment IDs.

New candidate profiles use result schema v7. Submission validation requires the
effective archival proof (`pruned_height=0`), the encoded birthday height and
stock two-day scanner-start resolution, and fingerprints for all four generated
seeds including `recipient_sink`. The committed historical schema-v6 baseline is
non-submission evidence and is accepted only through the explicit
`validate-profile --legacy-v6` path.

Do not use old namespaces, copied wallet DBs, or `--fresh-data-dir`. The harness
locks the candidate namespace, rejects dirty PP/signer state, stores child logs
under the namespace, and terminates managed process groups on SIGINT/SIGTERM.

See `RUNBOOK.md` for protocol and recovery details.

## License

BSD-3-Clause. See `LICENSE`.
