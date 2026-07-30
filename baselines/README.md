# Baseline Status

`esmeralda_baseline.json` is the historical schema-v6 profile from uncapped run
`baseline-20260727T110514Z`. It contains all 27 benchmark cells but does not pass
the current schema-v7 submission gate. `esmeralda_baseline.summary.md` is its
deterministic human-readable summary.

```sh
cargo run --release --locked -- validate-profile --profile baselines/esmeralda_baseline.json --legacy-v6
cargo run --release --locked -- summarize-profile --profile baselines/esmeralda_baseline.json --out /tmp/esmeralda_baseline.summary.generated.md --legacy-v6
cmp -s /tmp/esmeralda_baseline.summary.generated.md baselines/esmeralda_baseline.summary.md
```

The harness measured at commit `769e75c365ed470fbb3d964df4f30d3763f0439e`.
Run provenance, integrity records, and the detailed scenario assessment are in
[`analysis/baseline-20260727-audit.md`](../analysis/baseline-20260727-audit.md).
