# wallet-benchmarks

A reproducible performance harness for the Tari wallets. It runs the same benchmark protocol
against three wallet implementations and writes a structured result profile:

| Mode | Wallet | Driven through |
|---|---|---|
| `old_wallet` | [`minotari_console_wallet`](https://github.com/tari-project/tari) | gRPC, with the harness owning the process lifecycle |
| `new_wallet` | [`minotari-cli`](https://github.com/tari-project/minotari-cli) | the `minotari` crate as a library, signing offline with `sign_locked_transaction` |
| `payment_processor` | [`minotari_payment_processor`](https://github.com/tari-project/minotari_payment_processor) in front of a `minotari` daemon | HTTP, with the harness owning both process lifecycles — see [Mode 3](#mode-3-the-payment-processor) |

Scenarios implemented: **B0, S0, S1, S2, S3, S4, S5, S6, S7**.

## How it is put together

Two traits carry the whole design:

* `WalletMode` (`src/modes/mod.rs`) — everything a benchmark needs from a wallet: reset to a
  birthday, scan to the tip, read balances and UTXOs, construct-and-broadcast a transaction,
  follow it to confirmation. `old_wallet.rs` implements it over gRPC, `new_wallet.rs` over the
  library, `payment_processor.rs` over HTTP to two processes. Scenarios never see the
  difference.
* `Scenario` (`src/scenarios/mod.rs`) — one type per protocol step, each returning a
  `ScenarioReport`. A run is the cross product of the requested modes and scenarios.

`new_wallet` sends only through the library's own paths: `TransactionSender` for a single
output, and `FundLocker::lock` followed by `OneSidedTransaction::create_unsigned_transaction`
when a transaction pays several recipients (the S1 fan-out and the S5 batch arm), which is the
same pair the CLI's `create-unsigned-transaction` and the REST endpoint use. Signing is
`sign_locked_transaction`; broadcast is `WalletHttpClient::submit_transaction`.

Supporting modules: `binaries` (version pinning for everything the harness spawns),
`config` (every knob, copied into the profile), `seeds` (seed generation,
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
export HARNESS_SIGNER_PASSPHRASE='another long random passphrase'   # mode 3 only
```

`benchmark.toml` holds no secrets. The wallet password comes from the environment variable
named by `password_env`, the offline signer's keystore passphrase from the one named by
`payment_processor.signer_passphrase_env`, and the seed words are written to `seed_file`
(`data/seeds.json`, git-ignored, mode 0600).

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
cargo run --release -- run --modes payment_processor
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

## Mode 3: the payment processor

`payment_processor` drives the real
[`minotari_payment_processor`](https://github.com/tari-project/minotari_payment_processor)
service. It is three processes the harness starts and stops, plus one it never invokes:

```
harness
  |  spawn + lifecycle                       HTTP
  +--> minotari daemon (payment receiver) <-------+
  |      holds the wallet, the seed and the       |   GET  /accounts/{name}/balance
  |      scan loop; serves lock_funds             |   POST /accounts/{name}/lock_funds
  |  spawn + lifecycle                            |
  +--> minotari_payment_processor  ---------------+
  |      5 background workers, SQLite payments.db
  |      spawns minotari_offline_signer once per batch
  |
  |  HTTP: POST /v1/payment-batches, GET /v1/payments/{id}, GET /v1/events
  |
  +--> base node (operator supplied, HTTP RPC)
```

### The four binaries

| Binary | Built from | Pinned at | Version asserted at start-up |
|---|---|---|---|
| `minotari` (daemon) | [minotari-cli](https://github.com/tari-project/minotari-cli) | `60fde7b3` | `--version`, `pins.minotari` |
| `minotari_payment_processor` | [minotari_payment_processor](https://github.com/tari-project/minotari_payment_processor) | `4c5d3cf` | `GET /health/version`, `pins.payment_processor` |
| `minotari_offline_signer` | [tari](https://github.com/tari-project/tari) | `9f5adb7` (v5.3.0-pre.3) | `--version`, `pins.offline_signer` |
| `minotari_console_wallet` (mode 1) | [tari](https://github.com/tari-project/tari) | `v5.6.0-pre.1` | `--version`, `pins.console_wallet` |

`tools/fetch.sh` builds all four. Note that it checks tari out **twice**, at two different
revisions, and that this is not something to tidy up — see the version straddle below.

### Version straddle

Mode 3 spans two tari generations:

| Component | tari pin | Offline-signing payload format |
|---|---|---|
| `minotari_payment_processor` | `766f80cc` | 4.0.0 |
| `minotari_offline_signer` | `9f5adb7` (v5.3.0-pre.3) | 4.0.0 |
| `minotari` daemon | `tari_transaction_components 5.6.0-pre.1` | 5.0.0 |
| this harness | `tari_* 5.6.0-pre.1` | 5.0.0 |

This works because the 4.0.0 payload never crosses the seam: the processor does not call the
daemon's `create_unsigned_transaction`, it calls `lock_funds` and builds the unsigned payload
locally with its own pin, so the payload only ever travels processor → signer and both are on
4.0.0. A signer built from tari v5.4.0 or later speaks 5.0.0, cannot read those payloads, and
fails **every** signature.

What *does* cross the generations is `WalletOutput`: the daemon serialises locked UTXOs at
5.6.0-pre.1 and the processor deserialises them at `766f80cc`. At these two pins the two
representations were compared field by field — same 17 serde-visible fields, same order, same
names, same two `#[serde(skip)]`, every reachable field type identical, and no serde-surface
change in `tari_script` or in the reachable part of `tari_common_types`. **That compatibility is
incidental, not guaranteed. Re-verify it whenever either pin moves.**

### Signer keystore

The signer keeps its keys in the OS keyring, encrypted with the passphrase named by
`payment_processor.signer_passphrase_env`, so the keystore survives every data-directory wipe.
The harness initialises it from the seed the daemon account was created from, and re-points it
(`clear` then `init seed-words`) whenever the seed changes — B0 runs on a different seed from
S0–S7, and a keystore holding the wrong spend key fails every batch in exactly the way a
missing keystore does, only less visibly. It holds no measurable state.

### `--api-disable-auth`

The daemon is started with `--api-disable-auth`, bound to loopback. It token-gates every route
otherwise, and the processor builds its client as `MinotariConfiguration { base_path,
..default() }` with no credential set, so every processor → daemon call would be a 401. That API
can spend funds; loopback-only is the whole of the mitigation, and this mode should not be run
on a host where that is not enough.

### What mode 3's numbers mean

* **The wallet is the daemon.** The processor has no scanner, no seed and no balance. B0, S2,
  S3, S6 and S7 measure the daemon's scan loop — a separate process on its own schedule, which
  is a genuinely different measurement from `new_wallet`'s in-process scan.
* **Send scenarios run under concurrent scan load, by construction.** The daemon scans while it
  serves, and this cannot be turned off: the processor reads
  `GET /accounts/{name}/balance` before every lock and a stale balance fails the lock.
* **The pipeline is serial.** One `unsigned_tx_creator`, one signer subprocess at a time. S4
  should show a flat throughput ceiling with linearly growing latency, and that ceiling is the
  result rather than something to tune around.
* **One `POST /v1/payment-batches` is one transaction.** `POST /v1/payments` is never used:
  `batch_creator` coalesces every pending payment for an account into a single batch per cycle
  and would silently merge transactions a scenario meant to keep apart. `MAX_BATCH_SIZE` is 100,
  which is also this mode's `max_outputs_per_tx`, and `S5_M = 100` is exactly one batch — arm A
  cannot be widened further without changing `s5_k`.
* **The worker sleeps are harness configuration, disclosed.** They are set to 1 s against the
  processor's own defaults of 600/15/10/15/60 s, and recorded in the profile, so a scenario
  measures the wallet rather than the poll loop. Same rule as `new_wallet.db_pool_size`.
  `CONFIRMATION_CHECKER_REQUIRED_CONFIRMATIONS` is set to `params.c_min` rather than the
  processor's default of 10, so "confirmed" means the same thing in all three modes.
* **`MAX_INPUT_COUNT_PER_TX` is 400.** Above it the processor performs a CoinJoin cycle that
  changes the UTXO count in ways S1's arithmetic does not expect. At `volume_target = 512` that
  is reachable; a round that trips it halts S1 and is recorded, not tuned around.
* **Retries stay on.** The processor retries a batch up to `MAX_RETRIES = 10` internally. That
  is the wallet's own behaviour, not harness-level retry, so it is left enabled — but a wedged
  batch burns ten worker cycles before failing, which the scenario timeouts have to absorb.

### What mode 3 cannot report

These are findings, recorded in every mode 3 scenario report under `mode_findings`:

1. The processor emits no `TransactionBroadcast` or `TransactionMempoolDetected` event, despite
   declaring both in `EventType` — only six of its ten declared variants are ever written. So
   `broadcast_secs` is `null` for this mode, with the reason, rather than zero.
2. It persists no per-state timestamps: payments and batches carry only `created_at` and
   `updated_at`, overwritten on every transition. The `events` table is the sole transition
   history, and it is what the profile's per-transaction `enrichment.timeline` comes from.
3. It exposes no transaction id. Live polling goes by `payment_id`; on-chain identity is
   reachable only through `payref`, and only once a payment has confirmed. So fees, kernel
   excesses and confirmation heights are recovered after each scenario rather than at send time.
   The fee is read off the transaction the daemon stored, because neither service reports one,
   and that same stored transaction is the only source of the kernel's excess *signature* — the
   daemon's API reports the excess and nothing else, and the base node's transaction query is
   keyed on the signature, so without it the chain cannot be asked about the transaction at all.
   Fees that arrive this late are folded back into the scenario's balance reconciliation, which
   would otherwise report mode 3's whole fee spend as an unexplained shortfall. Everything else
   derived from a fee waits with them: a mode answers `reports_fee_at_send()` and a scenario
   that has fee-derived figures recomputes them in `finalise`, after enrichment. That is where
   S1's per-round value conservation is checked for this mode, where S4's per-level cost comes
   from, and where S5's `fee_multiplier` — the answer to "does batching save fees", and the
   reason the scenario exists — is computed. Checked against the fee reported at send time they
   would all be zero, and S1 would fail every round of the one mode the comparison is about.
   A `payref` only exists once a payment has confirmed, so a scenario that ended with a payment
   still in flight gets some of its fees back and not others. Every figure derived from a fee
   therefore carries a `fees_source`, and `enrichment_incomplete` means the total beside it is a
   floor: S1 skips value conservation for that round, S4 names the levels it applies to, and S5
   withholds `fee_multiplier` rather than divide one floor by another.
4. The daemon has no unspent-output endpoint — `lock_funds` is the only place raw UTXOs appear
   in its API, and calling that to read a balance would reserve funds. The harness reads the
   daemon's `outputs` table directly instead (SQLite in WAL mode, read-only connection).
5. `--confirmation-window` is not a flag on the `daemon` subcommand, only a `[wallet]` config
   key, so the harness writes the daemon's `config.toml` itself rather than letting the daemon
   materialise its default. Otherwise the daemon would run at a confirmation window of 3
   whatever `params.c_min` said.
6. The payment processor repository carries no LICENSE file and no `license` field in
   `Cargo.toml`, which blocks an external contributor from redistributing a patched build. Out
   of scope here; worth resolving upstream.

## What comes out

`results/profile-<timestamp>.json` contains:

* **environment** — CPU model, cores, RAM, OS, architecture, base node URL and whether it is
  local or remote.
* **versions** — harness version, the `minotari_console_wallet` version it talked to, the
  pinned `minotari-cli` git revision, every spawned binary with the version it reported and the
  pin it was checked against, base node URL and sync state.
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

**Pending.** The four profiles in `results/` are superseded and not comparable to a current run:
they were produced at `fee_rate = 1`, on the old `minotari-cli` pin, without S7 and without
`payment_processor`. See `results/README.md`.

A new baseline needs a funded Esmeralda run over the full 3 × 9 matrix: fund the three seeds
printed by `prepare` at `A_fund` (mode 3's into the daemon account's address), then
`cargo run --release -- run` at the reference defaults, and commit the resulting
`results/profile-<timestamp>.json`. The profile records the environment it was produced on and
the version every binary reported, so it identifies itself.

## The scenarios

| Id | What it does | Verifies |
|---|---|---|
| B0 | Genesis scan of a never-funded wallet | 0 UTXOs, 0 balance |
| S0 | Fresh wallet on the funded seed; resolves `H_birth`; waits for the funding UTXO | at least one spendable UTXO, worth at least `A_fund` |
| S1 | 6 doubling rounds (1 input → 2 outputs) then a fan-out round (1 input → 8 outputs), scaled to the starting UTXO count | UTXO count per round, `total_after == total_before − fees` (checked after enrichment for a mode that cannot price its own sends) |
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
* Resource figures are attributed per process. Each scan report carries `scan.processes` — a
  breakdown of peak RSS and peak CPU per named process, headline first, each flagged with
  whether it also covers the harness. `old_wallet` reports one external process, `new_wallet`
  one process that *is* the harness, and `payment_processor` reports the daemon as the headline
  (it is the wallet) with the payment processor alongside it. A single number for a
  three-process mode would describe neither.
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
