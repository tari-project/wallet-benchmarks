# Tari Wallet Benchmark Result

- Run ID: `run-1785531043893614543-467991`
- Profile: `final`
- Complete: `true`
- Network: `esmeralda`
- Measurement commit: `cfcedaaf0f15ded7b0e0826c91a890d2898006e1`
- Export commit: `cfcedaaf0f15ded7b0e0826c91a890d2898006e1`
- Harness worktree clean: measurement=`true`, export=`true`
- Selected scan node: `http://127.0.0.1:18242` (`v5.5.0`; `local`)
- Independent authority: `https://rpc.esmeralda.tari.com` (`remote`)

| Mode | Scenario | Execution | Outcome | Median ms (ok) | Median ms (all) | API accepted | Chain confirmed | Rejected | Stalled | Timed out | Successes | Failures |
|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| old_wallet | B0 | completed | success | 10555077 | 10555077 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S0 | completed | success | 1056 | 1056 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S1 | completed | success | 1385282 | 1385282 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| old_wallet | S2 | completed | failure | — | 10887072 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S3 | completed | failure | — | 119059 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S4 | completed | failure | — | 1358442 | 9 | 237 | 11 | 0 | 0 | 237 | 11 |
| old_wallet | S5 | completed | failure | — | 417445 | 99 | 99 | 1 | 0 | 0 | 99 | 1 |
| old_wallet | S6 | completed | failure | — | 10069053 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S7 | completed | failure | — | 57043 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | B0 | completed | failure | — | 277380 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | S0 | completed | failure | — | 2276 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | S1 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S2 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S3 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S4 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S5 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S6 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| new_wallet | S7 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | B0 | completed | failure | — | 266209 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| payment_processor | S0 | completed | success | 7646 | 7646 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S1 | completed | failure | — | 3180103 | 3 | 3 | 0 | 0 | 0 | 3 | 2 |
| payment_processor | S2 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S3 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S4 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S5 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S6 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| payment_processor | S7 | blocked_prerequisite | unavailable | — | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Confirmed top-level transactions: **466**
