# Result profiles

## The four profiles in this directory are superseded

`profile-20260818T205821Z.json`, `profile-20260819T131044Z.json`,
`profile-20260819T203233Z.json` and `profile-20260820T131248Z.json` were produced before the
harness reached full coverage. They are kept so the work that went into them is not lost and so
their scan numbers stay quotable for the two modes they cover, but **they are not a baseline and
they are not comparable with a current run.** Four things changed underneath them:

* **`fee_rate` was 1, and is now 5.** A fee of 1 has been measured taking about an hour to mine
  on Esmeralda against minutes at 5, so those runs' confirmation timings are largely a
  measurement of how long a barely-above-minimum-fee transaction waits for a miner. 5 is also
  what the `minotari` daemon and the payment processor both default to, and mode 3 cannot be run
  at anything else without misrepresenting it, so one fee across all three modes is what makes
  them comparable at all.
* **The `minotari-cli` pin was `84c79e9c`, and is now `60fde7b3`.** The five intervening commits
  are all transaction-reservation and idempotency fixes — atomic send idempotency, a replayed
  send no longer tearing down or double-spending a reservation, teardown guarded on the
  reservation still being pending, the unlocker kept off claimed rows. That is exactly what S4
  stresses, so `new_wallet`'s S4 numbers in these files describe software that no longer exists.
* **S7 did not exist.** These profiles have no birthday rescan at the second checkpoint, so
  neither `T_scan(S7) − T_scan(S3)` nor `T_scan(S6) / T_scan(S7)` can be derived from them, and
  the second checkpoint has only its genesis scan — which is dominated by the blocks that
  predate the wallet.
* **`payment_processor` did not exist.** They are 2 × 8, not 3 × 9.

The profile format also moved: `broadcast_secs` is now nullable with a reason,
`scan.resource_sample_includes_harness` has been replaced by a per-process
`scan.processes` breakdown, and `versions.binaries` records what every spawned binary reported.
Older profiles still deserialise — the new fields default and the removed one is ignored — so
`cargo run -- show results/<file>` still renders them, but the columns it prints for them are
not the columns a current run produces.

## The current baseline

There is none yet. Producing one needs a funded Esmeralda run over the full 3 × 9 matrix; see
the README's "Baseline profile" section. When it lands, this file should say which file it is
and leave the paragraphs above in place.
