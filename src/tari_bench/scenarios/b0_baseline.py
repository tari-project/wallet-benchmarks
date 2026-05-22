"""
B0 — Baseline Scan (empty wallet).

Floor cost of block-walk + view-key check with nothing to store.
"""

from __future__ import annotations

import asyncio
import logging

from .base import ScenarioBase

log = logging.getLogger(__name__)


class BaselineScan(ScenarioBase):
    """B0: Baseline scan — empty wallet, no history."""

    @property
    def name(self) -> str:
        return "B0"

    async def run(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        # Fresh seed, fresh data dir, genesis scan
        scan_metrics = await self.wallet.scan(from_genesis=True)
        result.blocks_scanned = scan_metrics.get("blocks_scanned", 0)
        result.blocks_per_sec = scan_metrics.get("blocks_per_sec", 0.0)
        result.chain_tip_start = scan_metrics.get("H_tip_start", 0)
        result.chain_tip_end = scan_metrics.get("H_tip_end", 0)
        # Verify empty
        bal = await self.wallet.get_balance()
        result.balance_final = bal
        result.balance_delta = -bal  # expected 0
        result.balance_reconciled = bal == 0

    async def verify(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        assert result.balance_reconciled, "B0: wallet should have 0 balance"
        assert result.blocks_scanned >= 0, "B0: blocks_scanned should be >= 0"
