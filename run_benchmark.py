#!/usr/bin/env python3
"""
Tari Wallet Benchmarks — CLI entry point.

Usage:
    tari-bench --mode old,new,payment --scenario ALL --config config.yaml
    tari-bench --mode new --scenario B0,S0
    tari-bench --mode payment              # runs all scenarios
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Tari Wallet Performance Benchmark Harness",
    )
    parser.add_argument(
        "--mode", "-m",
        default="all",
        help="Wallet mode(s): old, new, payment, all (comma-sep, default: all)",
    )
    parser.add_argument(
        "--scenario", "-s",
        default="ALL",
        help="Scenario filter: B0,S0,S1,…,ALL (comma-sep, default: ALL)",
    )
    parser.add_argument(
        "--config", "-c",
        default="config.yaml",
        help="Path to YAML config file (default: config.yaml)",
    )
    parser.add_argument(
        "--log-level",
        default="INFO",
        choices=["DEBUG", "INFO", "WARNING", "ERROR"],
        help="Logging level (default: INFO)",
    )
    parser.add_argument(
        "--result", "-r",
        default="",
        help="Output JSON path (overrides config.result_file)",
    )
    args = parser.parse_args()

    logging.basicConfig(
        level=getattr(logging, args.log_level.upper()),
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%H:%M:%S",
    )

    from tari_bench.config import BenchmarkConfig
    from tari_bench.runner import BenchmarkRunner

    # Load config
    if Path(args.config).exists():
        cfg = BenchmarkConfig.from_yaml(args.config)
    else:
        cfg = BenchmarkConfig.from_env()

    # Parse --mode
    if args.mode == "all":
        cfg.wallet_modes = ["old", "new", "payment"]
    else:
        cfg.wallet_modes = [m.strip() for m in args.mode.split(",")]

    # Parse --scenario
    if args.scenario and args.scenario.upper() != "ALL":
        cfg.scenarios_filter = [s.strip().upper() for s in args.scenario.split(",")]

    if args.result:
        cfg.result_file = args.result

    runner = BenchmarkRunner(cfg)
    runner.run()


if __name__ == "__main__":
    main()
