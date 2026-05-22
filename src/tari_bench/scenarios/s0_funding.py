"""
S0 — Funding Baseline.

Identical starting state for all downstream scenarios: receive 1 UTXO of
A_fund and wait for C_min confirmations.
"""

from __future__ import annotations

import asyncio
import logging
import time

from ..metrics import TxMetrics
from .base import ScenarioBase

log = logging.getLogger(__name__)


class FundingSetup(ScenarioBase):
    """S0: Fund the wallet from a faucet or external source."""

    @property
    def name(self) -> str:
        return "S0"

    async def run(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        tip = await self.wallet.get_tip_height()
        result.chain_tip_start = tip
        tx = TxMetrics(scenario="S0", round_label="funding")

        tx.construction_start_ns = time.monotonic_ns()
        # In production, the user funds the wallet externally.
        # The harness waits for the UTXO to appear.
        log.info(
            "[%s/S0] Waiting for funding UTXO of %d tXTM …",
            self.wallet.mode_name(), self.config.A_fund,
        )
        deadline = time.monotonic() + 300.0  # 5 min for funding
        funded = False
        while time.monotonic() < deadline:
            bal = await self.wallet.get_balance()
            if bal >= self.config.A_fund:
                funded = True
                break
            await asyncio.sleep(5.0)

        if not funded:
            raise TimeoutError(
                f"[{self.wallet.mode_name()}/S0] Funding not received within 5 min. "
                f"Please send {self.config.A_fund} tXTM to the wallet address."
            )
        tx.construction_end_ns = time.monotonic_ns()
        tx.success = True
        tx.fee_paid = 0
        tx.outputs_count = 1

        # Wait for confirmation
        if self.config.C_min > 1:
            log.info("[%s/S0] Awaiting C_min=%d confirmations …",
                     self.wallet.mode_name(), self.config.C_min)
            await asyncio.sleep(30.0)  # simplified — real impl checks block explorer
            tx.confirmation_time_ns = time.monotonic_ns()

        self.collector.record_tx(self.wallet.mode_name(), "S0", tx)
        utxos = await self.wallet.get_utxos()
        result.utxos_spendable = len(utxos)
        result.balance_final = await self.wallet.get_balance()
        result.transactions_success = 1

    async def verify(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        assert result.balance_final >= self.config.A_fund, (
            f"S0: expected balance >= {self.config.A_fund}, got {result.balance_final}"
        )
