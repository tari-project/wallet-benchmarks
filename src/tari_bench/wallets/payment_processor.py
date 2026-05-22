"""
Payment Processor wallet mode.

Extends NewWallet with batch 1-to-many transaction capability.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
from typing import Any

from .new_wallet import NewWallet
from .base import TxResult

log = logging.getLogger(__name__)


class PaymentProcessor(NewWallet):
    """Batch-1-to-many wallet mode built on top of minotari-cli."""

    def mode_name(self) -> str:
        return "payment"

    async def send_batch(
        self,
        recipients: list[tuple[str, int]],
        batch_size: int = 10,
        fee: int | None = None,
    ) -> list[TxResult]:
        """
        Send 1-to-many batch transactions.

        Sends batch_size outputs per transaction. Returns a TxResult per tx.
        """
        results: list[TxResult] = []
        for i in range(0, len(recipients), batch_size):
            batch = recipients[i : i + batch_size]
            t0 = time.monotonic_ns()
            try:
                args = ["transaction", "send", "--json"]
                for addr, amount in batch:
                    args += ["--output", f"{addr}:{amount}"]
                if fee is not None:
                    args.append(f"--fee={fee}")
                out = await self._cli(*args)
                t1 = time.monotonic_ns()
                data = json.loads(out)
                tx_id = data.get("transaction_id", "")
                results.append(TxResult(
                    tx_id=tx_id,
                    success=True,
                    fee_paid=data.get("fee", 0),
                    construction_ns=t1 - t0,
                    inputs=1,
                    outputs=len(batch),
                ))
            except Exception as exc:
                results.append(TxResult(
                    success=False,
                    error=str(exc),
                    construction_ns=time.monotonic_ns() - t0,
                ))
            await asyncio.sleep(0.1)  # small gap to avoid flooding
        return results
