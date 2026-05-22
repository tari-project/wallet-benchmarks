"""
S5 — Payment Processor Throughput.

Headline comparison: batch 1-to-N vs individual 1-to-1 sends.
"""

from __future__ import annotations

import asyncio
import logging
import time

from ..metrics import TxMetrics
from .base import ScenarioBase

log = logging.getLogger(__name__)


class PaymentProcessorThroughput(ScenarioBase):
    """S5: compare batch vs individual throughput."""

    @property
    def name(self) -> str:
        return "S5"

    async def run(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        result.chain_tip_start = await self.wallet.get_tip_height()
        M = self.config.S5_M
        K = self.config.S5_K

        # Generate M recipient addresses (self-sends)
        recipients = [("self", 1) for _ in range(M)]

        # --- Arm A: Batch ---------------------------------------------------
        log.info("[%s/S5] Arm A — batch (K=%d)", self.wallet.mode_name(), K)
        batch_start = time.monotonic()
        batch_txs: list[asyncio.Task] = []
        for i in range(0, M, K):
            batch = recipients[i : i + K]
            batch_txs.append(
                asyncio.ensure_future(self._send_batch(batch, i // K))
            )
        batch_results = await asyncio.gather(*batch_txs)
        t_batch = time.monotonic() - batch_start
        await asyncio.sleep(self.config.C_min * 5)

        # --- Arm B: Individual ----------------------------------------------
        log.info("[%s/S5] Arm B — individual (%d txs)", self.wallet.mode_name(), M)
        indiv_start = time.monotonic()
        indiv_txs: list[asyncio.Task] = []
        for i in range(M):
            indiv_txs.append(
                asyncio.ensure_future(
                    self.wallet.create_and_broadcast(
                        outputs=[("self", 1)], fee=None,
                    )
                )
            )
        indiv_results = await asyncio.gather(*indiv_txs)
        t_individual = time.monotonic() - indiv_start

        # --- Compute metrics ------------------------------------------------
        if t_batch > 0:
            result.throughput_multiplier = round(t_individual / t_batch, 4)
        else:
            result.throughput_multiplier = None
        result.batch_count = len(batch_txs)
        result.individual_count = len(indiv_txs)
        result.wall_clock_seconds = round(t_individual, 3)

        log.info("[%s/S5] Throughput multiplier: %.2fx (batch=%.1fs, indiv=%.1fs)",
                 self.wallet.mode_name(),
                 result.throughput_multiplier or 0,
                 t_batch, t_individual)
        result.chain_tip_end = await self.wallet.get_tip_height()

    async def _send_batch(self, batch: list, batch_idx: int) -> None:
        """Submit a batch transaction (payment processor mode uses batch API)."""
        if self.wallet.mode_name() == "payment":
            # PaymentProcessor has send_batch
            if hasattr(self.wallet, "send_batch"):
                await self.wallet.send_batch(batch, batch_size=len(batch))
                return
        # Fallback: individual sends
        for addr, amt in batch:
            await self.wallet.create_and_broadcast(outputs=[(addr, amt)])

    async def verify(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        assert result.throughput_multiplier is not None
