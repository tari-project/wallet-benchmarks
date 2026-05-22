"""
Benchmark runner — orchestrates scenarios across wallet modes.

Coordinates the full test protocol:
  1. Create wallet instances for requested modes
  2. Run scenarios in strict order (B0 → S0 → … → S7)
  3. Collect metrics and produce final JSON report
"""

from __future__ import annotations

import asyncio
import json
import logging
import shutil
import sys
import time
from pathlib import Path
from typing import Any

from .config import BenchmarkConfig
from .metrics import MetricsCollector, BenchmarkReport
from .wallets import WalletBase, OldWallet, NewWallet, PaymentProcessor
from .scenarios import (
    ScenarioBase,
    BaselineScan,
    FundingSetup,
    UtxoBuildUp,
    ScanFromGenesis,
    ScanFromBirthday,
    ConcurrentConstruction,
    PaymentProcessorThroughput,
)

log = logging.getLogger(__name__)

SCENARIO_ORDER = ["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"]


def _wallet_factory(mode: str, config: BenchmarkConfig) -> WalletBase:
    """Create a wallet instance for the given mode."""
    if mode == "old":
        return OldWallet(config)
    elif mode == "new":
        return NewWallet(config)
    elif mode == "payment":
        return PaymentProcessor(config)
    raise ValueError(f"Unknown wallet mode: {mode}")


def _build_scenarios(
    wallet: WalletBase,
    config: BenchmarkConfig,
    collector: MetricsCollector,
    filter_list: list[str] | None,
) -> list[ScenarioBase]:
    """Build scenario instances in the correct order."""
    w = wallet
    c = config
    col = collector

    registry: dict[str, ScenarioBase] = {
        "B0": BaselineScan(w, c, col),
        "S0": FundingSetup(w, c, col),
        "S1": UtxoBuildUp(w, c, col),
        "S2": ScanFromGenesis(w, c, col, label="S2"),
        "S3": ScanFromBirthday(w, c, col, birthday=0, label="S3"),
        "S4": ConcurrentConstruction(w, c, col),
        "S5": PaymentProcessorThroughput(w, c, col),
        "S6": ScanFromGenesis(w, c, col, label="S6"),
        "S7": ScanFromBirthday(w, c, col, birthday=0, label="S7"),
    }

    if filter_list:
        selected = [s for s in SCENARIO_ORDER if s in filter_list]
    else:
        selected = SCENARIO_ORDER

    return [registry[s] for s in selected if s in registry]


class BenchmarkRunner:
    """Main benchmark orchestrator."""

    def __init__(self, config: BenchmarkConfig) -> None:
        self.config = config
        self.collector = MetricsCollector()

    async def run_all(self) -> BenchmarkReport:
        """Run all modes × scenarios."""
        for mode in self.config.wallet_modes:
            log.info("=" * 60)
            log.info("Wallet mode: %s", mode.upper())
            log.info("=" * 60)

            wallet = _wallet_factory(mode, self.config)
            data_dir = Path(self.config.data_dir) / mode
            if data_dir.exists():
                shutil.rmtree(str(data_dir))
            data_dir.mkdir(parents=True, exist_ok=True)

            try:
                await wallet.spawn(str(data_dir), self.config.wallet_seed_words)
                ready = await wallet.wait_ready()
                if not ready:
                    log.error("[%s] Wallet not ready — skipping", mode)
                    continue

                scenarios = _build_scenarios(
                    wallet, self.config, self.collector,
                    self.config.scenarios_filter,
                )
                for scenario in scenarios:
                    await scenario.run_full()

            except Exception as exc:
                log.error("[%s] Fatal error: %s", mode, exc)
            finally:
                await wallet.shutdown()

        return self.collector.build_report(self.config)

    def run(self) -> None:
        """Synchronous entry point."""
        report = asyncio.run(self.run_all())

        # Write result file
        result_path = Path(self.config.result_file)
        result_path.parent.mkdir(parents=True, exist_ok=True)
        with open(result_path, "w") as f:
            f.write(report.to_json())
        log.info("Results written to %s", result_path)
        print("\n" + report.to_json())
