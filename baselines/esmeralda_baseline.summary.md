# Tari Wallet Benchmark Result

- Run ID: `run-1786332549837382208-39335`
- Profile: `final`
- Complete: `true`
- Network: `esmeralda`
- Measurement commit: `fec343e82cabd4a08e5640d3ff9a853663f1b99b`
- Export commit: `fec343e82cabd4a08e5640d3ff9a853663f1b99b`
- Harness worktree clean: measurement=`true`, export=`true`
- Selected scan node: `http://127.0.0.1:18242` (`v5.5.0`; `local`)
- Independent authority: `https://rpc.esmeralda.tari.com` (`remote`)

| Mode | Scenario | Execution | Outcome | Median ms (ok) | Median ms (all) | API accepted | Chain confirmed | Rejected | Stalled | Timed out | Successes | Failures |
|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| old_wallet | B0 | completed | success | 11329075 | 11329075 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S0 | completed | success | 1085 | 1085 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S1 | completed | success | 1628201 | 1628201 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| old_wallet | S2 | completed | success | 11466054 | 11466054 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S3 | completed | success | 128051 | 128051 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S4 | completed | failure | — | 4500000 | 9 | 0 | 9 | 0 | 239 | 0 | 248 |
| old_wallet | S5 | completed | failure | — | 1807464 | 99 | 0 | 1 | 0 | 99 | 0 | 100 |
| old_wallet | S6 | completed | failure | — | 13263069 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S7 | completed | failure | — | 708052 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | B0 | completed | failure | — | 287462 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | S0 | completed | failure | — | 2374 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | S1 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S2 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S3 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S4 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S5 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S6 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S7 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | B0 | completed | failure | — | 266443 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| payment_processor | S0 | completed | success | 7744 | 7744 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S1 | completed | failure | — | 1800049 | 1 | 0 | 0 | 0 | 1 | 0 | 1 |
| payment_processor | S2 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S3 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S4 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S5 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S6 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S7 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Confirmed top-level transactions: **127**

## Findings

- **27-cell outcome coverage: old_wallet=5 successful/4 measured non-success/0 blocked; new_wallet=0 successful/2 measured non-success/7 blocked; payment_processor=1 successful/2 measured non-success/6 blocked** (`limited_by_prerequisites`): measurement-coverage Recommendation: Do not infer performance for blocked cells; resolve the recorded prerequisites before comparison: new_wallet/S1, new_wallet/S2, new_wallet/S3, new_wallet/S4, new_wallet/S5, new_wallet/S6, new_wallet/S7, payment_processor/S2, payment_processor/S3, payment_processor/S4, payment_processor/S5, payment_processor/S6, payment_processor/S7.
- **Measured non-success outcomes occurred in old_wallet/S4, old_wallet/S5, old_wallet/S6, old_wallet/S7, new_wallet/B0, new_wallet/S0, payment_processor/B0, payment_processor/S1.** (`observed`): measured-non-success-cells Recommendation: Interpret each non-success through its recorded terminal outcome, wallet error, and observation evidence; do not replace it with a retry.
- **Post-S5 scan time divided by empty-wallet B0: old_wallet=1.171x.** (`measured`): scan-slowdown Recommendation: Compare these ratios with the absolute scan durations and the recorded fixed chain targets; ratios alone do not separate chain walk from wallet-state cost.
- **S5 wall-time ratios: unavailable; fee-per-recipient ratios: unavailable.** (`unavailable`): s5-payment-processor-comparison Recommendation: A ratio above 1 favors payment-processor batching for the named metric; report wall time and fee efficiency together rather than treating either as the sole result.
