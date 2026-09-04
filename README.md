# wallet-benchmarks

A reproducible performance harness for the Tari wallets. It runs the same benchmark protocol
against two wallet implementations and writes a structured result profile:

| Mode | Wallet | Driven through |
|---|---|---|
| `old_wallet` | [`minotari_console_wallet`](https://github.com/tari-project/tari) | gRPC, with the harness owning the process lifecycle |
| `new_wallet` | [`minotari-cli`](https://github.com/tari-project/minotari-cli) | the `minotari` crate as a library, signing offline with `sign_locked_transaction` |

Scenarios implemented: **B0, S0, S1, S2, S3, S4, S5, S6, S7**.

## How it is put together

Two traits carry the whole design:

* `WalletMode` (`src/modes/mod.rs`) — everything a benchmark needs from a wallet: reset to a
  birthday, scan to the tip, read balances and UTXOs, construct-and-broadcast a transaction,
  follow it to confirmation. `old_wallet.rs` implements it over gRPC, `new_wallet.rs` over the
  library. Scenarios never see the difference.
* `Scenario` (`src/scenarios/mod.rs`) — one type per protocol step, each returning a
  `ScenarioReport`. A run is the cross product of the requested modes and scenarios.

`new_wallet` sends only through the library's own paths: `TransactionSender` for a single
output, and `FundLocker::lock` followed by `OneSidedTransaction::create_unsigned_transaction`
when a transaction pays several recipients (the S1 fan-out and the S5 batch arm), which is the
same pair the CLI's `create-unsigned-transaction` and the REST endpoint use. Signing is
`sign_locked_transaction`; broadcast is `WalletHttpClient::submit_transaction`.

Supporting modules: `config` (every knob, copied into the profile), `seeds` (seed generation,
birthday rewriting, funding addresses), `chain` (base node access shared by both modes),
`metrics` (measurements and the result profile), `sysprobe` (environment disclosure, peak RSS
and CPU), `runner` (orchestration).

The harness does not retry, back off, throttle or pre-partition UTXOs. A wallet that stalls,
locks itself out or rejects a transaction under load is measured doing so.

## Requirements

* Rust (stable, 2021 edition or newer) and `protoc` (`brew install protobuf`, `apt install
  protobuf-compiler`) — the gRPC client is generated at build time.
* A synced Tari base node reachable over HTTP RPC. The public Esmeralda gateway
  (`https://rpc.esmeralda.tari.com`) works; a local node gives much less noisy scan numbers.
* The wallet binaries, built at their pinned revisions by `tools/fetch.sh`:

  ```bash
  ./tools/fetch.sh                    # all of them
  ./tools/fetch.sh console_wallet     # or one at a time
  ```

  Nothing is committed. Each binary's version is asserted against the `[pins]` section of
  `benchmark.toml` before a run starts, and every version observed is recorded in the profile,
  so a profile always names the software that produced it. A binary reporting the wrong version
  refuses the run rather than quietly producing a profile that claims a pin it never had.
* Funds on Esmeralda: one UTXO of at least `A_fund` (10,000 tXTM by default) per mode.

## Running it

### 1. Configure

```bash
git clone https://github.com/tari-project/wallet-benchmarks
cd wallet-benchmarks
cp benchmark.example.toml benchmark.toml
$EDITOR benchmark.toml          # base_node_url, old_wallet.binary, parameters
export HARNESS_WALLET_PW='a long random passphrase'
```

`benchmark.toml` holds no secrets. The wallet password comes from the environment variable
named by `password_env`, and the seed words are written to `seed_file` (`data/seeds.json`,
git-ignored, mode 0600).

### 2. Create the seeds and fund them

```bash
cargo run --release -- prepare
```

This generates, per mode, a **funded** seed (S0–S6) and a **never-funded** seed (B0), and
prints the addresses:

```
Fund each of these addresses with a single UTXO of at least 10000000000 µT (A_fund is a minimum):

  old_wallet               f24j27UtLSZ...
  new_wallet               f2KE9gjEEgG...

These wallets must never be funded (they are the B0 baseline):
  old_wallet-baseline      f25czsoLGu3...
  new_wallet-baseline      f2EbDMG1CJy...
```

Send at least `A_fund` to each funded address, from anywhere — faucet, miner wallet, a
previous run's sweep-back. Both the amount and the UTXO count are floors: the profile records
what each wallet actually received, and S1 scales its build-up to the number of UTXOs it finds
(the reference protocol starts from one, so a run that starts from four ends on 2,048 UTXOs
instead of 512 and says so). Funding is explicitly not part of the measurement. Wait until each
funding transaction is at least `C_min` blocks deep.

Re-running `prepare` never rotates an existing seed, so the money stays where it is.
`cargo run --release -- addresses --json` reprints the addresses at any time.

### 3. Check the plan, then run

```bash
cargo run --release -- plan                       # parameters and scenario list, no wallets touched
cargo run --release -- run                        # every mode, B0 and S0-S7, in order
cargo run --release -- run --modes new_wallet --scenarios B0,S0,S1
```

A run prints a `mode x scenario` matrix, then summary tables — time per scenario with the
ratio between the modes, what each scan cost, what each transaction scenario cost, and the
computed deltas — and writes `results/profile-<timestamp>.json`. The same tables can be
re-rendered from any saved profile without re-running:

```bash
cargo run --release -- show results/profile-<timestamp>.json
``` The scenarios form a chain — S1 builds the UTXO set S2 and
S3 rescan, S4 and S5 spend it, S6 and S7 rescan what they left — so when one fails, the rest of that
mode is skipped rather than run against a wallet state the protocol never asked for. The other
mode still runs.

Expect a full default-scale run to take hours. S1 alone broadcasts 127 transactions and waits
for each round to reach `C_min`, and every scan scenario walks the chain from genesis.

### Running at reduced scale

For a shake-out run, shrink the protocol — `volume_target` must stay equal to
`2^doubling_rounds * fanout_outputs_per_tx`:

```toml
[params]
doubling_rounds = 3
fanout_outputs_per_tx = 4
volume_target = 32
concurrent_batches = [4, 8]
s5_m = 20
s5_k = 5
a_fund = 100000000
```

Reduced-scale numbers are not comparable with the reference profile. The profile records every
parameter used, so which is which is never in doubt.

## What comes out

`results/profile-<timestamp>.json` contains:

* **environment** — CPU model, cores, RAM, OS, architecture, base node URL and whether it is
  local or remote.
* **versions** — harness version, the `minotari_console_wallet` version it talked to, the
  pinned `minotari-cli` git revision, base node URL and sync state.
* **config** — the full configuration verbatim.
* **reports** — per mode and scenario: wall clock, chain tips at start and end, balances,
  UTXO counts, fees, success and failure counts with error strings, balance reconciliation,
  scan metrics (`T_scan`, blocks/sec, blocks scanned, outputs found, peak RSS, peak CPU) and
  every individual transaction (construction time, broadcast time, confirmation time, fee,
  outcome).
* **deltas** — `T_scan(S2) − T_scan(B0)`, `T_scan(S6) − T_scan(S2)`,
  `T_scan(S6) / T_scan(B0)`, `T_scan(S7) − T_scan(S3)`, `T_scan(S6) / T_scan(S7)` and the S5
  throughput multiplier, per mode.

### Baseline profile

No profile is committed yet: one has to come from a real run against Esmeralda with funded
wallets. Produce it with `cargo run --release -- run` on the reference defaults and commit the
resulting `results/profile-<timestamp>.json` together with the environment it was produced on
(the profile records that itself).

## The scenarios

| Id | What it does | Verifies |
|---|---|---|
| B0 | Genesis scan of a never-funded wallet | 0 UTXOs, 0 balance |
| S0 | Fresh wallet on the funded seed; resolves `H_birth`; waits for the funding UTXO | at least one spendable UTXO, worth at least `A_fund` |
| S1 | 6 doubling rounds (1 input → 2 outputs) then a fan-out round (1 input → 8 outputs), scaled to the starting UTXO count | UTXO count per round, `total_after == total_before − fees` |
| S2 | Wipe, rescan from genesis | rediscovers the pre-wipe UTXO set and balance |
| S3 | Wipe, rescan from the seed birthday | same, from `H_birth` |
| S4 | 8/16/32/64/128 concurrent construct-and-broadcast calls | per-tx timings, success rate, max serialisation gap |
| S5 | Pays the same 100 recipients as 10 batch txs and as 100 individual txs | `T_batch`, `T_individual`, multiplier and fee per recipient — the multiplier only when both arms paid every recipient |
| S6 | Wipe, rescan from genesis after S4 and S5 | rediscovers the post-S5 state |
| S7 | Wipe, rescan from the seed birthday after S4 and S5 | same, from `H_birth` |

Each checkpoint is rescanned twice, from genesis and from the birthday: S2/S3 after the
build-up, S6/S7 after the concurrency and throughput scenarios. The genesis figure is dominated
by the blocks that predate the wallet, which no amount of wallet history changes, so
`T_scan(S7) - T_scan(S3)` is what actually says what S4 and S5 cost a rescan.

Notes on what the numbers mean:

* The console wallet builds and broadcasts inside a single gRPC call, so for `old_wallet` the
  whole call is reported as construction time and broadcast time is zero. For `new_wallet` the
  two are measured separately.
* `new_wallet` runs inside the harness process, so its peak RSS and CPU include the harness.
  Each scan report says so in `resource_sample_includes_harness`.
* S4 never truncates construction. The scenario is "how fast can this wallet push N
  transactions out", so the calls run to completion however long the wallet takes, and
  `s4_budget_secs` bounds only the wait for confirmations afterwards; a level that used its
  whole budget on construction is flagged `budget_exhausted`. Because a queued send fails once
  it exceeds the pool's checkout timeout, `new_wallet.db_checkout_timeout_secs` has to stay
  above `params.s4_budget_secs` — start-up refuses the combination rather than let the
  plumbing give up before the scenario does.
* The wallet database is opened with a pool sized for the S4 ramp, not the library's own
  `DB_POOL_SIZE = 5`, so a ramp to 128 measures the wallet rather than the pool.
  `new_wallet.db_pool_size = 0` sizes it to the ramp's peak and records the value used.
  S4 also reports `infrastructure_failures` per level, so plumbing failures stay separate from
  the wallet's own rejections.

  This used to matter far more: before `minotari-cli` 84c79e9 a single send held **three**
  connections at once, so any pool below `3 x peak concurrency` deadlocked rather than queued
  — at N = 64 against 64 connections, 6001 s of construction and zero transactions. A send now
  holds one, threaded through selection, locking and the write transaction.
* Chain **tip heights** come from the base node, so both modes are described against the same
  ruler, and tips are recorded at the start and end of every scenario for drift disclosure.
  **Confirmation does not.** Whether a transaction has reached `C_min` is asked of the wallet,
  because the wallet is what decides when the money can be spent again, and a round waits for
  two things — both from the wallet:
  1. its own verdict that the transaction is confirmed (the console wallet's
     `MINED_CONFIRMED` status, the library wallet's `confirmed_height` being set), and
  2. `C_min` blocks of depth, measured against the height *that wallet* has scanned to.

  The verdict alone would ignore `C_min` whenever the wallet's own threshold is the lower of
  the two; the depth alone would call a transaction settled while the wallet still refuses to
  spend against it, which is what had S1 firing rounds into "Funds are still pending".
* Between rounds, S1 additionally waits for the wallet to say it will spend again and records
  that as `settle_secs`. The base node is never consulted about a transaction.
* Neither wallet accepts a scan-start height from the harness: each reads the birthday
  encoded in the seed words it was given (the console wallet from its stored seed,
  `minotari-cli` from the `birthday` column filled in when the account is created). Scan
  scenarios therefore re-encode the mnemonic with birthday 0 for a genesis scan and pass the
  original mnemonic for a birthday scan. The entropy is untouched, so it is the same wallet
  with the same address either way. Both wallets then step two days back from that birthday
  before they begin, which is what `scan.start_height` reports; `h_birth` in S0 is the
  unadjusted birthday height.
* A console wallet rescan is only comparable once it has *reconciled* what it recovered. The
  recovery imports every output the seed ever received; without the reconciliation an S2
  rescan reports the wallet's entire history as unspent — 639 UTXOs and 8x the balance for a
  wallet holding 512. `has_done_initial_validation` is not the signal: it means "some faux
  validation finished since scanning did", and after a 639-output recovery it was already true
  14 ms in with nothing marked spent. So the harness calls the wallet's own
  `RevalidateAllTransactions` (with `OutputValidationMode::Revalidate`) and then waits for the
  UTXO set to hold still for `validation_stable_secs`, since revalidation announces no
  completion. That wait is reported as `validation_secs` and included in `T_scan`; a set that
  never settles is recorded as `validation_completed: false` rather than failing the run.
* `new_wallet` confirmation is read from the wallet's outputs table with a narrow query, not
  through the library's own helpers. Two upstream bugs make the tidier routes unusable: under
  `ScanMode::Full` the transaction monitor that maintains `completed_transactions` sits behind
  an unsatisfiable guard in `unified_scan_loop`, so those records never leave `Broadcast`; and
  `db::get_output_by_id` cannot map its own rows, because the scanner writes `mined_timestamp`
  with a UTC offset that will not parse back into a `NaiveDateTime`. Both are commented in
  `modes/new_wallet.rs` and both are worth fixing upstream.
* A rescan is compared against the wallet it replaced at the *same height*, and counts every
  output the wallet holds including ones a pending transaction has locked. Both matter: a
  rescan finishes minutes after the snapshot it is checked against, and a lock is a
  reservation rather than a disposal. Nothing releases an expired lock in a harness run —
  that is the daemon's `tasks::unlocker` — so a send that never confirms leaves its input
  locked for the rest of the run, and counting locked outputs as gone made an S6 rescan look
  like it had invented one (652 against 651, with the balance identical to the µT).
* An arm of S5 that does not pay every recipient still gets its duration recorded, but no
  throughput or fee multiplier is derived from it: ten batch calls rejected in 14 ms against a
  completed individual arm would read as a 25,000x speed-up.
* S1 asks each wallet for half (or an eighth) of a specific UTXO, but neither wallet lets a
  caller choose its inputs. What the wallet actually selects is what gets measured; a round
  that does not produce the expected UTXO count halts S1 and is reported.

## Development

```bash
cargo test        # unit tests, no network and no wallets required
cargo clippy --all-targets
```

The `minotari-cli` dependency is pinned to a git revision in `Cargo.toml`;
`metrics::MINOTARI_CLI_REV` and `tools/fetch.sh` record the same revision, and every profile
carries it. Change all three together.

The pin is currently `60fde7b3`, bumped from `84c79e9c`. The five intervening commits are all
transaction-reservation and idempotency fixes — atomic send idempotency, a replayed send no
longer tearing down or double-spending a reservation, teardown guarded on the reservation still
being pending, and the unlocker kept off claimed rows. That is precisely the ground S4 stresses
under concurrency, so **expect `new_wallet`'s S4 numbers to move** against any profile taken on
the old pin. The two are not comparable.

## Licence

BSD-3-Clause.
