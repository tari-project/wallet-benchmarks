"""
Metrics collection and result reporting.

Collects per-scenario, per-mode metrics and assembles the final structured JSON
result profile per the bounty specification.
"""

from __future__ import annotations

import json
import platform
import time
import os
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from typing import Any


# ---------------------------------------------------------------------------
# Per-transaction metrics
# ---------------------------------------------------------------------------

@dataclass
class TxMetrics:
    """Metrics for a single transaction."""
    tx_id: str = ""
    scenario: str = ""
    round_label: str = ""
    construction_start_ns: int = 0
    construction_end_ns: int = 0
    broadcast_time_ns: int = 0
    confirmation_time_ns: int = 0
    fee_paid: int = 0
    fee_unit: str = "tXTM"
    success: bool = False
    error: str | None = None
    inputs_count: int = 0
    outputs_count: int = 0


# ---------------------------------------------------------------------------
# Per-scenario results
# ---------------------------------------------------------------------------

@dataclass
class ScenarioResult:
    """Aggregated result for one scenario in one wallet mode."""
    mode: str = ""
    scenario: str = ""
    wall_clock_seconds: float = 0.0
    chain_tip_start: int = 0
    chain_tip_end: int = 0
    blocks_scanned: int = 0
    blocks_per_sec: float = 0.0

    # UTXO / balance
    utxos_found: int = 0
    utxos_spendable: int = 0
    balance_final: int = 0
    balance_delta: int = 0          # expected − observed
    balance_reconciled: bool = True

    # Fees
    total_fees_paid: int = 0
    transactions_total: int = 0
    transactions_success: int = 0
    transactions_failed: int = 0

    # System
    peak_rss_mb: float = 0.0
    peak_cpu_pct: float = 0.0

    # S4 specific
    max_serialization_gap_ms: float = 0.0

    # S5 specific
    throughput_multiplier: float | None = None
    batch_count: int = 0
    individual_count: int = 0

    # Raw tx metrics (omitted from final output by default)
    tx_metrics: list[TxMetrics] = field(default_factory=list)

    notes: str = ""


# ---------------------------------------------------------------------------
# Hardware / environment disclosure
# ---------------------------------------------------------------------------

@dataclass
class EnvironmentInfo:
    """System disclosure for reproducibility."""
    cpu_model: str = ""
    cpu_cores: int = 0
    ram_gb: float = 0.0
    disk_type: str = ""
    os: str = ""
    network_to_base_node: str = ""          # local | remote
    console_wallet_version: str = ""
    cli_version: str = ""
    base_node_version: str = ""
    benchmark_tool_version: str = "1.0.0"
    run_timestamp_utc: str = ""

    @classmethod
    def capture(cls, cfg: Any = None) -> EnvironmentInfo:
        """Auto-capture current environment."""
        try:
            import psutil
            ram = psutil.virtual_memory().total / (1024**3)
            cpu_model = _cpu_name()
            cores = psutil.cpu_count(logical=True) or 0
        except Exception:
            ram = 0.0
            cpu_model = platform.machine()
            cores = os.cpu_count() or 0

        return cls(
            cpu_model=cpu_model,
            cpu_cores=cores,
            ram_gb=round(ram, 1),
            disk_type="ssd" if _is_ssd() else "hdd",
            os=f"{platform.system()} {platform.release()}",
            network_to_base_node="",
            run_timestamp_utc=datetime.now(timezone.utc).isoformat(),
        )


def _cpu_name() -> str:
    """Extract CPU model name (Linux / macOS)."""
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    return line.split(":")[1].strip()
    except Exception:
        pass
    try:
        import subprocess
        out = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"], timeout=2
        ).decode().strip()
        if out:
            return out
    except Exception:
        pass
    return platform.processor() or platform.machine()


def _is_ssd() -> bool:
    """Heuristic: check if / or the cwd is on an SSD."""
    try:
        import subprocess
        dev = subprocess.check_output(
            ["stat", "-f", "-c", "%d", "."], timeout=2
        ).decode().strip()
        out = subprocess.check_output(
            ["lsblk", "-o", "NAME,ROTA", "-n"], timeout=2
        ).decode()
        for line in out.splitlines():
            if dev in line and "1" in line.split():
                return False
        return True
    except Exception:
        return True  # assume ssd


# ---------------------------------------------------------------------------
# Final report
# ---------------------------------------------------------------------------

@dataclass
class BenchmarkReport:
    """Complete structured result profile."""
    environment: EnvironmentInfo = field(default_factory=EnvironmentInfo)
    configuration: dict[str, Any] = field(default_factory=dict)
    results: dict[str, dict[str, ScenarioResult]] = field(default_factory=dict)
    # results[mode][scenario] = ScenarioResult

    def to_json(self, indent: int = 2) -> str:
        """Serialize to JSON string."""
        def _convert(obj: Any) -> Any:
            if isinstance(obj, (ScenarioResult, TxMetrics, EnvironmentInfo)):
                return {k: _convert(v) for k, v in asdict(obj).items()
                        if k != "tx_metrics" or obj == obj}  # strip raw tx data
            if isinstance(obj, dict):
                return {k: _convert(v) for k, v in obj.items()}
            if isinstance(obj, list):
                return [_convert(v) for v in obj]
            if isinstance(obj, float):
                return round(obj, 4)
            return obj

        raw = _convert(asdict(self))
        return json.dumps(raw, indent=indent, ensure_ascii=False)


# ---------------------------------------------------------------------------
# Collector — used at runtime
# ---------------------------------------------------------------------------

class MetricsCollector:
    """Collects metrics during benchmark execution."""

    def __init__(self) -> None:
        self._results: dict[str, dict[str, ScenarioResult]] = {}
        # {mode: {scenario: ScenarioResult}}

    def get_result(self, mode: str, scenario: str) -> ScenarioResult:
        """Get or create a result container."""
        if mode not in self._results:
            self._results[mode] = {}
        if scenario not in self._results[mode]:
            self._results[mode][scenario] = ScenarioResult(mode=mode, scenario=scenario)
        return self._results[mode][scenario]

    def record_tx(self, mode: str, scenario: str, tx: TxMetrics) -> None:
        """Append a single transaction metric."""
        r = self.get_result(mode, scenario)
        r.tx_metrics.append(tx)
        r.transactions_total += 1
        if tx.success:
            r.transactions_success += 1
            r.total_fees_paid += tx.fee_paid
        else:
            r.transactions_failed += 1

    def build_report(self, cfg: Any) -> BenchmarkReport:
        """Assemble the final report."""
        env = EnvironmentInfo.capture(cfg)
        return BenchmarkReport(
            environment=env,
            configuration=cfg.to_dict() if hasattr(cfg, "to_dict") else {},
            results=self._results,
        )
