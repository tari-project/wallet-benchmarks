"""
Abstract base class for benchmark scenarios.

Every scenario follows:
  1. setup() — prepare state
  2. run() — execute the test
  3. verify() — assert acceptance criteria
  4. teardown() — clean up
Returns a ScenarioResult.
"""

from __future__ import annotations

import abc
import logging
import time
from typing import Any

from ..metrics import ScenarioResult
from ..wallets.base import WalletBase

log = logging.getLogger(__name__)


class ScenarioBase(abc.ABC):
    """Abstract scenario."""

    def __init__(self, wallet: WalletBase, config: Any, collector: Any) -> None:
        self.wallet = wallet
        self.config = config
        self.collector = collector
        self._result: ScenarioResult | None = None

    @property
    @abc.abstractmethod
    def name(self) -> str:
        """Return scenario code e.g. 'B0', 'S0'."""
        ...

    async def run_full(self) -> ScenarioResult:
        """Execute the full lifecycle."""
        log.info("[%s] Starting scenario %s", self.wallet.mode_name(), self.name)
        start = time.monotonic()
        try:
            await self.setup()
            await self.run()
            await self.verify()
        except Exception as exc:
            log.error("[%s/%s] Failed: %s", self.wallet.mode_name(), self.name, exc)
            result = self.collector.get_result(self.wallet.mode_name(), self.name)
            result.notes = f"FAILED: {exc}"
        finally:
            await self.teardown()
        elapsed = round(time.monotonic() - start, 3)
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        result.wall_clock_seconds = elapsed
        tip = await self.wallet.get_tip_height()
        result.chain_tip_end = tip
        rss, cpu = await self.wallet.get_peak_resources()
        result.peak_rss_mb = round(rss, 1)
        result.peak_cpu_pct = round(cpu, 1)
        log.info("[%s] %s done in %.1fs", self.wallet.mode_name(), self.name, elapsed)
        return result

    async def setup(self) -> None: ...
    async def run(self) -> None: ...
    async def verify(self) -> None: ...
    async def teardown(self) -> None: ...
