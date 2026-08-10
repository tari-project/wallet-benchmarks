# Baseline Status

`esmeralda_baseline.json` is the schema-v7 submission profile from uncapped run
`run-1786332549837382208-39335`. It contains all 27 benchmark cells and passes
the current submission gate. `esmeralda_baseline.summary.md` is its deterministic
human-readable summary.

```sh
cargo run --release --locked -- validate-profile --profile baselines/esmeralda_baseline.json --submission
cargo run --release --locked -- summarize-profile --profile baselines/esmeralda_baseline.json --out /tmp/esmeralda_baseline.summary.generated.md
cmp -s /tmp/esmeralda_baseline.summary.generated.md baselines/esmeralda_baseline.summary.md
```

The harness measured at commit `fec343e82cabd4a08e5640d3ff9a853663f1b99b`.
After all 27 cells were checkpointed, the final independent-authority query
failed because the WSL network route disappeared. The final profile changes only
the profile-completion and end-anchor fields through the hash-bound manifest in
[`analysis/reconciliation/esmeralda_baseline.schema-v7.correction.json`](../analysis/reconciliation/esmeralda_baseline.schema-v7.correction.json).
CI reconstructs the committed profile from the raw checkpoint and verifies the
captured authority evidence.

The prior uncapped schema-v6 result remains available as
`esmeralda_baseline.schema-v6.json` with its deterministic
`esmeralda_baseline.schema-v6.summary.md`. It is historical, non-submission
evidence and validates only through the explicit legacy path:

```sh
cargo run --release --locked -- validate-profile --profile baselines/esmeralda_baseline.schema-v6.json --legacy-v6
```

Its provenance, correction evidence, and detailed assessment remain documented
in [`analysis/baseline-20260727-audit.md`](../analysis/baseline-20260727-audit.md).
