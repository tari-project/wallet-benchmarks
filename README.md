# Tari Wallet Benchmarks

Reproducible wallet performance test harness for the [Tari](https://tari.com) protocol.

> **Bounty**: [#1 — Reproducible Wallet Performance Test Harness](https://github.com/tari-project/wallet-benchmarks/issues/1)  
> **Tier**: L — 150,000 XTM

## Overview

This harness measures `minotari-cli` wallet performance across three modes:

| Mode | Description |
|------|-------------|
| **Old Wallet** (`minotari_console_wallet`) | gRPC-based interaction, subprocess lifecycle management |
| **New Wallet** (minotari-cli library) | Offline signing + HTTP RPC broadcast via base node |
| **Payment Processor** | Batch 1-to-many transaction capability |

## Test Scenarios

| Code | Scenario | What it measures |
|------|----------|-----------------|
| **B0** | Baseline Scan | Floor cost of empty-wallet block walk |
| **S0** | Funding Baseline | Receive 1 UTXO, wait for C_min confirmations |
| **S1** | UTXO Build-up | 6 doubling rounds → fan-out to 512 UTXOs. Per-tx construction & fee cost |
| **S2** | Scan from Genesis (CP1) | Rediscover 512 UTXOs from genesis after S1 |
| **S3** | Scan from Birthday (CP1) | Scan from birthday after S1 |
| **S4** | Concurrent Construction | Ramp 8→16→32→64→128 concurrent sends. No retry, no backoff |
| **S5** | Payment Processor | Batch 1-to-10 (10 txs) vs individual 1-to-1 (100 txs). Throughput multiplier |
| **S6** | Scan from Genesis (CP2) | Final genesis scan after S5 |
| **S7** | Scan from Birthday (CP2) | Final birthday scan after S5 |

## Prerequisites

- Python 3.11+
- Tari Esmeralda testnet access (base node running)
- `minotari_console_wallet` binary (for old wallet mode)
- `minotari-cli` binary (for new wallet / payment processor mode)
- A funded wallet with at least 10,000 tXTM (roughly 71% of one Esmeralda block reward)

## Quick Start

```bash
# 1. Install dependencies
pip install -r requirements.txt

# 2. Edit configuration
cp config.yaml config.local.yaml
# Edit config.local.yaml with your paths and base node URL

# 3. Run all scenarios on all wallet modes
python run_benchmark.py --config config.local.yaml --mode all

# 4. Run a specific mode + scenario
python run_benchmark.py --mode new --scenario B0,S0 --config config.local.yaml

# 5. View results
cat benchmark_result.json | python -m json.tool
```

## Configuration

All parameters are documented in `config.yaml`. Override via environment variables:

| Variable | Config field | Description |
|----------|-------------|-------------|
| `TARI_A_FUND` | `A_fund` | Funding amount in tXTM |
| `TARI_C_MIN` | `C_min` | Confirmation depth |
| `TARI_BASE_NODE_URL` | `base_node_url` | Base node RPC endpoint |
| `TARI_CONSOLE_WALLET_BIN` | `console_wallet_bin` | Path to wallet binary |
| `TARI_CLI_BIN` | `cli_bin` | Path to minotari-cli binary |
| `TARI_DATA_DIR` | `data_dir` | Working directory |
| `TARI_RESULT_FILE` | `result_file` | Output path |
| `TARI_DEBUG` | `debug` | Enable debug logging |

## Output

Results are written as a structured JSON file (`benchmark_result.json` by default) containing:

- **Environment**: CPU model, RAM, OS, disk type, binary versions
- **Configuration**: All parameters used for the run
- **Per-scenario metrics**: wall-clock, fees, success/failure, balance reconciliation
- **Computed deltas**: scan overhead, throughput multiplier

## Core Principle

This harness **measures** wallet performance — it does **not** engineer around wallet pain. UTXO locking, selection contention, stalls, and failed tx construction under concurrency are part of what is measured. We do not add retries, backoff, UTXO pre-partitioning, or throttling that would hide real wallet behaviour.

## Project Structure

```
├── run_benchmark.py          # CLI entry point
├── config.yaml               # Default configuration
├── pyproject.toml            # Python project metadata
├── requirements.txt          # Dependencies
├── Makefile                  # Common commands
├── results/                  # Baseline result profiles
│   └── baseline_profile.json # Example profile
└── src/tari_bench/           # Main package
    ├── config.py             # Configuration management
    ├── metrics.py            # Metrics collection & reporting
    ├── runner.py             # Test orchestrator
    ├── wallets/              # Wallet implementations
    │   ├── base.py           # Abstract base class
    │   ├── old_wallet.py     # gRPC wallet
    │   ├── new_wallet.py     # minotari-cli library
    │   └── payment_processor.py  # Batch tx processor
    └── scenarios/            # Test scenarios
        ├── base.py           # Scenario base class
        ├── b0_baseline.py
        ├── s0_funding.py
        ├── s1_utxo_build.py
        ├── s2_s6_scan_checkpoint.py
        ├── s4_concurrent.py
        └── s5_payment_processor.py
```
