"""
Configuration management for the Tari wallet benchmark harness.

All benchmark parameters have reference defaults from the bounty spec.
Configuration is loaded from:
  1. Default values in this module
  2. YAML config file (--config CLI flag)
  3. Environment variables (TARI_A_FUND, TARI_C_MIN, …)
"""

from __future__ import annotations

import os
import sys
import copy
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Configuration dataclass
# ---------------------------------------------------------------------------

@dataclass
class BenchmarkConfig:
    """Immutable-like configuration snapshot for one benchmark run."""

    # --- Funding & network ---
    A_fund: int = 10_000                     # tXTM — single funding UTXO
    C_min: int = 3                           # confirmation depth
    fee_rate: int = 1                        # pinned fee rate
    base_node_url: str = "http://127.0.0.1:18142"
    esmeralda_network: str = "esmeralda"
    faucet_url: str = ""                     # optional funding faucet

    # --- UTXO build-up (S1) ---
    volume_target: int = 512                 # target UTXOs after S1
    doubling_rounds: int = 6                 # from 1 → 64 UTXOs
    fanout_outputs_per_tx: int = 8           # from 64→512 in one round

    # --- Concurrency (S4) ---
    concurrent_batches: list[int] = field(
        default_factory=lambda: [8, 16, 32, 64, 128]
    )
    S4_T_budget_seconds: int = 900           # 15 minutes per batch size

    # --- Payment processor (S5) ---
    S5_M: int = 100                          # total recipients
    S5_K: int = 10                           # outputs per batch tx → 10 txs

    # --- Wallet binary versions (pinned) ---
    console_wallet_bin: str = "minotari_console_wallet"
    cli_bin: str = "minotari-cli"
    base_node_bin: str = "minotari_base_node"
    wallet_version_tag: str = "v0.59.0-pre.1"

    # --- Paths ---
    data_dir: str = "./.tari-bench-data"
    log_dir: str = "./.tari-bench-logs"
    result_file: str = "benchmark_result.json"
    wallet_seed_words: str = ""              # empty = generate fresh

    # --- Wallet mode filter (ALL = run all three) ---
    wallet_modes: list[str] = field(
        default_factory=lambda: ["old", "new", "payment"]
    )
    scenarios_filter: list[str] | None = None  # None = ALL

    # --- Misc ---
    debug: bool = False
    scenario_timeout_multiplier: float = 1.0

    # ------------------------------------------------------------------
    def to_dict(self) -> dict[str, Any]:
        """Return a plain dict (for JSON output)."""
        return asdict(self)

    @classmethod
    def from_yaml(cls, path: str | Path) -> BenchmarkConfig:
        """Load overrides from a YAML file."""
        import yaml
        p = Path(path)
        if not p.exists():
            return cls()
        with open(p) as f:
            raw = yaml.safe_load(f) or {}
        return cls._with_env_overrides(raw)

    @classmethod
    def from_env(cls) -> BenchmarkConfig:
        """Build config from env vars only."""
        return cls._with_env_overrides({})

    @classmethod
    def _with_env_overrides(cls, base: dict) -> BenchmarkConfig:
        """Apply TARI_* env vars on top of a base dict."""
        mapping = {
            "TARI_A_FUND": "A_fund",
            "TARI_C_MIN": "C_min",
            "TARI_FEE_RATE": "fee_rate",
            "TARI_BASE_NODE_URL": "base_node_url",
            "TARI_NETWORK": "esmeralda_network",
            "TARI_FAUCET_URL": "faucet_url",
            "TARI_CONSOLE_WALLET_BIN": "console_wallet_bin",
            "TARI_CLI_BIN": "cli_bin",
            "TARI_BASE_NODE_BIN": "base_node_bin",
            "TARI_DATA_DIR": "data_dir",
            "TARI_RESULT_FILE": "result_file",
            "TARI_SEED_WORDS": "wallet_seed_words",
            "TARI_BUDGET_SEC": "S4_T_budget_seconds",
            "TARI_DEBUG": "debug",
        }
        for env_key, attr in mapping.items():
            val = os.environ.get(env_key)
            if val is not None:
                if attr == "debug":
                    base[attr] = val.lower() in ("1", "true", "yes")
                elif attr in ("A_fund", "C_min", "fee_rate", "S4_T_budget_seconds", "S5_M", "S5_K"):
                    base[attr] = int(val)
                else:
                    base[attr] = val

        return cls(**{k: v for k, v in base.items() if hasattr(cls, k)})

    def scenario_timeout(self, base_seconds: int) -> float:
        return base_seconds * self.scenario_timeout_multiplier
