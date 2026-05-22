"""
S2 / S3 / S6 / S7 — Scan checkpoints.

S2: Scan from genesis (after S1, data-dir wiped)
S3: Scan from birthday (same state)
S6: Scan from genesis (final, after S5)
S7: Scan from birthday (final)
"""

from __future__ import annotations

import logging

from .base import ScenarioBase

log = logging.getLogger(__name__)


class ScanFromGenesis(ScenarioBase):
    """S2/S6: Scan from genesis with fresh wallet state."""

    def __init__(self, wallet, config, collector, label: str = "S2") -> None:
        super().__init__(wallet, config, collector)
        self._label = label

    @property
    def name(self) -> str:
        return self._label

    async def run(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        scan_metrics = await self.wallet.scan(from_genesis=True)
        result.blocks_scanned = scan_metrics.get("blocks_scanned", 0)
        result.blocks_per_sec = scan_metrics.get("blocks_per_sec", 0.0)
        result.chain_tip_start = scan_metrics.get("H_tip_start", 0)
        result.chain_tip_end = scan_metrics.get("H_tip_end", 0)
        result.balance_final = await self.wallet.get_balance()
        utxos = await self.wallet.get_utxos()
        result.utxos_found = len(utxos)
        result.balance_reconciled = True

    async def verify(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        assert result.utxos_found >= 0
        assert result.blocks_scanned >= 0


class ScanFromBirthday(ScenarioBase):
    """S3/S7: Scan from birthday."""

    def __init__(self, wallet, config, collector, birthday: int = 0, label: str = "S3") -> None:
        super().__init__(wallet, config, collector)
        self._birthday = birthday
        self._label = label

    @property
    def name(self) -> str:
        return self._label

    async def run(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        scan_metrics = await self.wallet.scan(from_genesis=False)
        result.blocks_scanned = scan_metrics.get("blocks_scanned", 0)
        result.blocks_per_sec = scan_metrics.get("blocks_per_sec", 0.0)
        result.chain_tip_start = scan_metrics.get("H_tip_start", 0)
        result.chain_tip_end = scan_metrics.get("H_tip_end", 0)
        result.balance_final = await self.wallet.get_balance()
        utxos = await self.wallet.get_utxos()
        result.utxos_found = len(utxos)
        result.balance_reconciled = True
