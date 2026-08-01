# Benchmark Runbook

## Safety Rules

- Use fresh seeds and a unique `paths.data_dir` for every candidate.
- Run B0 before any benchmark address is funded.
- Fund only one separate source wallet. Never fund benchmark addresses manually.
- Let `baseline-workflow` finish all three B0 scans before it automatically sends
  one transaction with three `A_fund` outputs to the fresh mode seeds.
- Never edit wallet databases, unlock outputs, retry scenario transactions, or
  normalize state between S4 and S5.
- A failed wallet operation is a result. A harness, provenance, schema, or
  environment failure invalidates the candidate.
- Keep AC power and sleep prevention enabled for a full run.

## Topology

The preferred final topology uses a pinned unpruned local Esmeralda node for
wallet scans and operations, plus public Esmeralda as independent authority.
The local node only reduces request latency; it cannot accelerate blocks or
`C_min`.

Set:

```toml
[network]
name = "esmeralda"
base_node_http_url = "http://127.0.0.1:18142"
authority_http_url = "https://rpc.esmeralda.tari.com"
mode1_base_node_service_peer = "PUBLIC_KEY::/ip4/127.0.0.1/tcp/18189"
```

Strict checks require both `pruning_horizon=0` and `pruned_height=0`,
selected/authority tip distance at most `C_min`, matching finalized hashes,
queryable funding headers, selected-chain unspent funding outputs, and current
wallet scanner heights.

### Local Node Setup

Keep the node database on a native Linux filesystem, not a Windows-mounted WSL
path such as `/mnt/c`. Set explicit absolute paths and initialize the pinned
node:

```sh
NODE_BIN="$(pwd)/tools/minotari_node"
NODE_ROOT="/absolute/native-linux/path/wallet-bench-esmeralda-node"

"$NODE_BIN" \
  --base-path "$NODE_ROOT" \
  --network esmeralda \
  --non-interactive-mode \
  --disable-splash-screen \
  --init
```

Stop it after it creates
`$NODE_ROOT/esmeralda/config/config.toml`. In that file, set the following
values in the existing sections:

```toml
[base_node]
use_libtor = false

[base_node.storage]
pruning_horizon = 0

[base_node.p2p.transport]
type = "tcp"
tcp.listener_address = "/ip4/127.0.0.1/tcp/18189"

[base_node.http_wallet_query_service]
port = 18142
listen_ip = "127.0.0.1"
external_address = "http://127.0.0.1:18142"
```

Before every start, fail closed if the persistent identity is missing or has
the wrong permissions; non-interactive mode otherwise creates a new identity.
Then start the node and leave it running:

```sh
NODE_ID="$NODE_ROOT/esmeralda/config/base_node_id_esmeralda.json"
test -s "$NODE_ID"
NODE_ID_MODE="$(
  stat -c '%a' "$NODE_ID" 2>/dev/null ||
  stat -f '%Lp' "$NODE_ID"
)"
test "$NODE_ID_MODE" = 600

"$NODE_BIN" \
  --base-path "$NODE_ROOT" \
  --network esmeralda \
  --non-interactive-mode \
  --disable-splash-screen
```

Extract only the public key (never print the complete identity file) and place
the resulting peer string in `network.mode1_base_node_service_peer`:

```sh
NODE_PUBLIC_KEY="$(
  perl -0777 -ne \
    'print "$1\n" if /"?public_key"?\s*:\s*"([0-9A-Fa-f]{64})"/' \
    "$NODE_ID"
)"
test "${#NODE_PUBLIC_KEY}" -eq 64
printf '%s::/ip4/127.0.0.1/tcp/18189\n' "$NODE_PUBLIC_KEY"
```

Wait for the node to synchronize fully. The readiness response must show
`is_synced=true`, `pruning_horizon=0`, and `pruned_height=0`.
Compare `http://127.0.0.1:18142/get_tip_info` with
`https://rpc.esmeralda.tari.com/get_tip_info`; `baseline-workflow` then performs
the authoritative archival, process-hash, tip-distance, and finalized-hash
checks. A stale node that reports `is_synced=true` still fails these checks.

## Build and Verify

Install the dependencies listed in `README.md`, then run:

```sh
scripts/fetch-minotari-cli.sh .bench-cache tools
scripts/fetch-payment-processor.sh .bench-cache tools
cargo fmt --check
cargo check --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
ast-grep scan
```

The fetch scripts pin upstream bases and verify their exact Git trees:

- minotari CLI/scanner upstream base: `tari-project/minotari-cli@360c4848a54d65fd710266233cc9277b0f785e74`
- console wallet: `tari-project/tari@9f5adb7183dc2ec285f5c8fae05f4be9735d9749`
- node: `tari-project/tari@v5.5.0`
- payment processor: `tari-project/minotari_payment_processor@f0572c98cbfac7377412dc6d4094c7d7dfc5de2c`

Cargo links the unmodified upstream Minotari library at that revision. Mode 2 S1
uses its ordinary `FundLocker` selection and wallet change path: each transaction
requests `N-1` self-payment outputs, change supplies output `N`, and the harness
records any multi-input selection or shape failure without retry.

The runtime Minotari CLI applies one operational hardening patch only:

- `patches/minotari-wallet-password-env.patch` enables Clap's environment support
  and accepts `MINOTARI_WALLET_PASSWORD`, so the payment receiver does not expose
  its wallet password in process arguments. It also changes the daemon's
  unauthenticated control API bind from `0.0.0.0` to `127.0.0.1`. It does not
  alter scanning, selection, transaction construction, signing, or broadcasting,
  and it hides the environment value from CLI help output.
  The harness supplies the payment receiver an explicit candidate-scoped
  Esmeralda configuration, so it does not create or inherit an ambient default
  configuration.

Minotari's control API has no upstream authentication. The runtime patch limits
it to loopback, but the source-wallet and benchmark daemons must run only on a
trusted, single-user host; do not expose or share the host during a live run.

The scanner correctness and exact-output behavioral patches used by the
2026-07-27 historical profile are no longer applied. Upstream premature scan
completion or transaction-shape failures are benchmark outcomes. The historical
profile and its hash-bound audit remain unchanged; do not describe it as evidence
from the current upstream-wallet build.

The PP build applies `patches/payment-processor-fee-rate.patch` to both ordinary
payment construction and self-spend consolidation. Upstream hard-codes `5`; the
bounty requires the exposed `benchmark.fee_rate` to control every mode, so the
patch makes both paths require the harness-provided `FEE_PER_GRAM`.

The pinned processor's `.env.sample` maps its worker cadences to
`15/15/10/15/60` seconds for batch creation, unsigned transaction creation,
transaction signing, broadcasting, and confirmation checking respectively.
The template, runtime environment, and result profile keep those fields
separate; submission validation pins the exact values. The payment receiver
wallet scan interval remains a separate `10s` setting.

Both fetchers verify known patch hashes, every final tree, and the
SHA-256 of the complete binary diff. Use `--verify-only` to perform those checks
without compiling. A normal PP build emits schema-v2 `tools/build-manifest.json`
with upstream bases, ordered patch hashes/result trees, and artifact hashes;
runtime preflight compares it against provenance embedded by `build.rs` and fails
closed on missing, extra, reordered, or changed claims. Both fetchers source the
same manifest-claim definitions, and CI exercises that emitter with placeholder
artifact hashes to detect drift without building the four upstream binaries.
On macOS, the fetchers ad-hoc re-sign copied executables before hashing them so
the manifest binds the runnable artifact bytes accepted by kernel code signing.

### Source Wallet

The only manually funded wallet is a `minotari` signing-wallet SQLite DB. It must
use the same `HARNESS_WALLET_PW` exported for the workflow and hold at least
`3 * A_fund` plus the shared transaction fee. Create one if needed:

```sh
read -r -s HARNESS_WALLET_PW
export HARNESS_WALLET_PW
export MINOTARI_WALLET_PASSWORD="$HARNESS_WALLET_PW"
export SOURCE_DB=/absolute/path/to/source-wallet.db
umask 077
mkdir -p "$(dirname "$SOURCE_DB")"
chmod 700 "$(dirname "$SOURCE_DB")"
tools/minotari --network esmeralda create \
  --database-path "$SOURCE_DB"
```

Start the wallet daemon in a second terminal so its API exposes the address and
keeps the source DB synchronized:

```sh
tools/minotari --network esmeralda daemon \
  --database-path "$SOURCE_DB" \
  --base-url http://127.0.0.1:18142 \
  --batch-size 1000 \
  --scan-interval-secs 10 \
  --api-port 9147
```

From the first terminal, read the returned address and fund it only with
Esmeralda L1 tXTM from another Esmeralda L1 wallet/faucet, or by mining against a
fully synchronized Esmeralda L1 node. Mainnet XTM held by Tari Universe and
Ootle L2 tTARI are different networks/assets and must not be sent to this
address. Wait until the balance endpoint reports at least `30000 T` plus fees,
then stop the daemon cleanly with Ctrl+C before the workflow opens the DB:

```sh
curl http://127.0.0.1:9147/accounts/default/address
curl http://127.0.0.1:9147/accounts/default/balance
sqlite3 "$SOURCE_DB" \
  "SELECT lower(hex(fingerprint)) FROM accounts WHERE friendly_name = 'default';"
```

Copy the returned Esmeralda address and hex fingerprint into the private,
ignored `harness.toml`:

```toml
[source_wallet]
database = "/absolute/path/to/source-wallet.db"
address = "THE_ESMERALDA_SOURCE_ADDRESS"
wallet_fingerprint_hex = "THE_HEX_DEFAULT_ACCOUNT_FINGERPRINT"
```

Funding commands canonicalize `--source-db` and require that exact configured
path, verify the address-to-fingerprint binding, and compare the database's
default-account fingerprint using a read-only SQLite connection. Before any
spend or recovery submission, the latest persisted wallet scan hash must also
match the selected node and independent Esmeralda authority at the same height.
Never configure a Tari Universe or other mainnet wallet here.

This is setup, not measurement. Do not send funds to the three generated mode
addresses: `baseline-workflow` does that only after every B0 scan completes and
records its measured outcome. The fourth
generated `recipient_sink` identity starts unfunded and is never opened or
scanned by the harness; every S4/S5 destination is recoverable from that seed.

## B0 and Funding

Start from `harness-prefunding.toml`. Change the candidate `paths.data_dir` and
the matching `modes.new_wallet_database`; set the local-node identity above. Do
not add funding records. The Mode 2 database must be inside `paths.data_dir`, so
one namespace lock and one recorded disk cover every measured wallet. For
Ubuntu/WSL, use a native-ext4 candidate namespace such as:

```toml
[paths]
data_dir = "/root/wallet-bench-state/candidates/candidate-YYYYMMDD-HHMMSS"

[modes]
new_wallet_database = "/root/wallet-bench-state/candidates/candidate-YYYYMMDD-HHMMSS/new-wallet/wallet.db"
```

Do not place the measured namespace under `/mnt/c`; keep only the source checkout
and final copied profile/summary on the Windows filesystem.

```sh
cp harness-prefunding.toml harness.toml
mkdir -p .secrets candidates
cargo run --release --locked -- addresses --config harness.toml --out .secrets/candidate.env
set -a; . .secrets/candidate.env; set +a
read -r -s HARNESS_WALLET_PW
export HARNESS_WALLET_PW

systemd-inhibit --what=sleep --why="Tari wallet benchmark" \
  cargo run --release --locked --features live-minotari -- baseline-workflow \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --s0-evidence candidates/s0-funding.json \
  --profile candidates/esmeralda-baseline.json \
  --summary candidates/esmeralda-baseline.md
```

`baseline-workflow` runs launch-invariant disk and build-manifest verification
once. Stage-sensitive port, endpoint, wallet-state, identity, and selected-chain
checks still run immediately before the stages they protect. `prepare-b0` binds
the harness commit, resolved protocol, timeouts, topology,
seed/address fingerprints, host/disk environment, endpoint identity, source
revisions, binary/patch hashes, and scan batch size. Because the console wallet
does not expose a stop-height API, its persisted completion cursor establishes
one B0 anchor height/hash. Both library-backed scans report success only if they
reach that exact same anchor. A scanner that returns below it is retained as a
completed wallet failure, without another invocation, so the rest of the matrix
can still measure that pinned wallet. `prepare-b0` requires
`scan_repetitions = 1` and fails before scanning otherwise. Every B0 scan must
recover zero outputs, balance, and history.

Canonical candidate commands require a clean harness worktree. The profile
records runtime cleanliness independently at measurement and final export, plus
whether the running executable itself was built from a clean worktree.
Submission requires all four observations to be clean, the build/runtime commit
to remain the same lowercase 40-hex Git commit, and the measurement/export
executable and build-manifest hashes to remain identical.

The command above is for Linux/WSL. On macOS, replace `systemd-inhibit ...` with
`caffeinate -dimsu --`.

If the process is interrupted after B0, resume the same funding transaction with
the standalone stage command:

```sh
cargo run --release --locked --features live-minotari -- fund-s0 \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --evidence-out candidates/s0-funding.json
```

The funding stage creates fresh benchmark wallet DBs at one measured birthday
and broadcasts one transaction with three recipients. It atomically records the
tx ID and broadcast timing before confirmation polling. Repeating the command
resumes that tx. Once confirmed, it synchronizes all three recipient DBs and
requires the strict one-output/`A_fund` readiness gate before proceeding.
Evidence records the mined height, `C_min` tip, signed-kernel fee, encoded
birthday day, unmodified `H_birth`, the stock two-day birthday offset, and the
effective scanner start height.

The pinned stock wallets scan from two days before their encoded birthday.
Birthday scan cells therefore report both protocol `blocks_scanned =
H_tip_end - H_birth` and `effective_blocks_scanned = H_tip_end -
birthday_start_height`; the latter discloses the extra work performed by the
wallet's unmodified safety window.

## Candidate Run

No funding values are copied into TOML. The run derives them from evidence and
executes strict harness, identity, and namespace checks before starting scenarios.

The workflow proceeds to the candidate run after independent funding
confirmation. Each wallet's recipient readiness is measured once in S0. The
standalone `prepare-b0`, `fund-s0`, and `run` commands remain available for
diagnosis and documented interrupted-funding recovery; each performs its own
launch checks when invoked separately.

Each non-B0 checkpoint pair (S2/S3 and S6/S7) captures one shared immutable
target height/hash before either cell starts. Every repetition in both cells
makes exactly one bounded invocation of the pinned scanner toward that same
target. A return below the target is preserved as the measured wallet outcome
and fails verification rather than triggering another invocation. Overshoot,
cursor-hash mismatch, target reorganization, or deadline expiry also fails the
cell. Completion tip/hash, scanner invocation count, progress, and `more_blocks`
are recorded as drift and implementation evidence. Recovered transaction IDs,
not only counts, are checked.

S0 failure blocks S1. S1 halts at the first failed round. S4 emits one observation
per requested call. S5 starts directly from disclosed post-S4 state. S6/S7 scan
the actual post-attempt state even if S5 is partial or fails. Per-transaction
confirmation time is the first independent `C_min` observation or null with a
reason; scenario wall time is never substituted.

## Publication

```sh
cargo run --release --locked -- validate-profile \
  --profile candidates/esmeralda-baseline.json --submission
cargo run --release --locked -- summarize-profile \
  --profile candidates/esmeralda-baseline.json \
  --out candidates/esmeralda-baseline.md
```

Submission validation requires completed, evidence-rich B0 and S0 wallet
outcomes, all cells present, no harness errors, exact canonical
configuration, coherent blocked prerequisites,
recomputed deltas, realistic transaction shapes, complete provenance,
`pruned_height=0`, the complete stock birthday resolution, and all four seed
fingerprints including `recipient_sink`. Honest S0 failure is valid when S1-S7
for that mode are blocked by S0; honest downstream wallet failure is also valid.

New candidates use result schema v7. The committed `baselines/esmeralda_baseline.json`
is the current submission profile. The historical schema-v6 baseline is preserved
as non-submission evidence and cannot pass the command above. Validate or
summarize that historical artifact only through the explicit legacy path:

```sh
cargo run --release --locked -- validate-profile \
  --profile baselines/esmeralda_baseline.schema-v6.json \
  --legacy-v6
```

Only after validation should the candidate JSON and summary replace the files in
`baselines/`. Never promote partial checkpoints or combine modes from separate
runs.

## Interrupted Runs

- Before S0 funding: rerun `prepare-b0` only in a new namespace if the checkpoint
  did not complete.
- During S0 confirmation: rerun the same `fund-s0` command.
- During measured scenarios: preserve the namespace and logs as diagnostics; do
  not resume or reuse mutated benchmark wallets for a candidate.
- After every mode checkpoint is complete: if only the final authority query
  fails, preserve the raw checkpoint and failure log. A reviewed correction may
  set only the final profile flag and captured end-anchor fields; bind it to the
  raw profile hash, retain the authority responses, and reproduce it in CI.
- SIGINT/SIGTERM triggers managed process-group shutdown. A stale
  `.wallet-bench.lock` means the namespace is non-canonical; preserve it and use a
  new namespace rather than deleting the lock to continue.

`fund-one-sided`, `scan-wallet`, `recover-mode1-wallet`, `sweep-mode1`, and
`query-tx` are noncanonical operator diagnostics/recovery commands. They are not
called by measured scenarios and must not be used to alter a candidate between
cells.

The harness source is licensed under BSD-3-Clause; see `LICENSE`.
