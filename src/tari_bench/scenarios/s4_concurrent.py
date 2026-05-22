"""
S4 — Concurrent Construction.

Measures what each wallet mode does under concurrent load.
No retry, no backoff, no UTXO pre-partitioning.
"""

from __future__ import annotations

import asyncio
import logging
import time

from ..metrics import TxMetrics
from .base import ScenarioBase

log = logging.getLogger(__name__)


class ConcurrentConstruction(ScenarioBase):
    """S4: ramp concurrent batches."""

    @property
    def name(self) -> str:
        return "S4"

    async def run(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        result.chain_tip_start = await self.wallet.get_tip_height()

        for n_conc in self.config.concurrent_batches:
            log.info("[%s/S4] Concurrent batch: N=%d",
                     self.wallet.mode_name(), n_conc)
            batch_start = time.monotonic()

            # Fire all construction calls in parallel
            tasks = []
            for i in range(n_conc):
                tasks.append(self._single_tx(i))

            done = await asyncio.wait(
                tasks,
                timeout=self.config.S4_T_budget_seconds,
                return_when=asyncio.ALL_COMPLETED,
            )

            batch_elapsed = time.monotonic() - batch_start
            successes = sum(
                1 for t in done[0]
                if t.result() is not None and t.result().success
            )
            failures = n_conc - successes

            log.info("[%s/S4] N=%d: %d OK, %d failed in %.1fs",
                     self.wallet.mode_name(), n_conc, successes, failures, batch_elapsed)

        result.chain_tip_end = await self.wallet.get_tip_height()
        result.transactions_total = sum(
            self.config.concurrent_batches
        )

    async def _single_tx(self, idx: int) -> TxMetrics | None:
        """Submit one concurrent transaction."""
        tx = TxMetrics(scenario="S4", round_label=f"conc_{idx}")
        tx.construction_start_ns = time.monotonic_ns()
        try:
            t_result = await self.wallet.create_and_broadcast(
                outputs=[("self", 1)],
                fee=None,
            )
            tx.construction_end_ns = time.monotonic_ns()
            tx.success = t_result.success
            tx.fee_paid = t_result.fee_paid
            tx.inputs_count = 1
            tx.outputs_count = 1
            self.collector.record_tx(self.wallet.mode_name(), "S4", tx)
            return tx
        except Exception as exc:
            tx.success = False
            tx.error = str(exc)
            self.collector.record_tx(self.wallet.mode_name(), "S4", tx)
            return tx

    async def verify(self) -> None:
        result = self.collector.get_result(self.wallet.mode_name(), self.name)
        assert result.chain_tip_end >= 0
