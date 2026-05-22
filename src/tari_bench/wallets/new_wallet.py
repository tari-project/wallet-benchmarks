"""
New wallet implementation — uses minotari-cli library with offline signing.

Interacts with the Tari base node via HTTP JSON-RPC for broadcast.
UTXO selection, transaction building, and signing are delegated to
the minotari-cli binary (subprocess) for maximum compatibility.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import signal
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

import httpx

from .base import WalletBase, UtxoInfo, TxResult

log = logging.getLogger(__name__)


class NewWallet(WalletBase):
    """Uses minotari-cli binary for offline-signing transactions."""

    def __init__(self, config: Any) -> None:
        self._cfg = config
        self._data_dir: str = ""
        self._cli_path = config.cli_bin
        self._seed_words: str = ""
        self._tip: int = 0
        self._peak_rss: float = 0.0
        self._peak_cpu: float = 0.0
        self._client = httpx.AsyncClient(timeout=60.0)

    def mode_name(self) -> str:
        return "new"

    async def spawn(self, data_dir: str, seed_words: str = "") -> None:
        self._data_dir = data_dir
        Path(data_dir).mkdir(parents=True, exist_ok=True)
        self._seed_words = seed_words
        log.info("New wallet initialised (data_dir=%s)", data_dir)

    async def wait_ready(self, timeout_s: float = 120.0) -> bool:
        try:
            tip = await self._rpc_call("get_tip_info")
            self._tip = tip.get("height", 0)
            return True
        except Exception as exc:
            log.warning("wait_ready: %s", exc)
            return False

    async def get_balance(self) -> int:
        out = await self._cli("wallet", "balance", "--json")
        data = json.loads(out)
        return data.get("available_balance", 0)

    async def get_utxos(self) -> list[UtxoInfo]:
        out = await self._cli("wallet", "list-utxos", "--json")
        data = json.loads(out)
        return [
            UtxoInfo(
                commitment=bytes.fromhex(u.get("commitment", "")),
                value=u.get("value", 0),
                height=u.get("height", 0),
            )
            for u in data.get("utxos", [])
        ]

    async def create_and_broadcast(
        self, outputs: list[tuple[str, int]], fee: int | None = None
    ) -> TxResult:
        t0 = time.monotonic_ns()
        try:
            args = ["transaction", "send", "--json"]
            for addr, amount in outputs:
                args += ["--output", f"{addr}:{amount}"]
            if fee is not None:
                args.append(f"--fee={fee}")
            out = await self._cli(*args)
            t1 = time.monotonic_ns()
            data = json.loads(out)
            tx_id = data.get("transaction_id", data.get("tx_id", ""))
            fee_paid = data.get("fee", 0)
            return TxResult(
                tx_id=tx_id,
                success=True,
                fee_paid=fee_paid,
                construction_ns=t1 - t0,
                inputs=1,
                outputs=len(outputs),
            )
        except Exception as exc:
            return TxResult(
                success=False,
                error=str(exc),
                construction_ns=time.monotonic_ns() - t0,
            )

    async def wait_for_confirmation(
        self, tx_id: str, depth: int = 3, timeout_s: float = 300.0
    ) -> bool:
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                out = await self._cli("transaction", "get", tx_id, "--json")
                data = json.loads(out)
                confirmations = data.get("confirmations", 0)
                if confirmations >= depth:
                    return True
            except Exception:
                pass
            await asyncio.sleep(2.0)
        return False

    async def scan(self, from_genesis: bool = False) -> dict:
        t0 = time.monotonic()
        tip_before = await self.get_tip_height()
        flag = "--from-genesis" if from_genesis else ""
        out = await self._cli("wallet", "scan", flag, "--json")
        t1 = time.monotonic()
        data = json.loads(out) if out.strip() else {}
        tip_after = await self.get_tip_height()

        return {
            "T_scan": round(t1 - t0, 3),
            "blocks_scanned": tip_after - tip_before,
            "blocks_per_sec": round((tip_after - tip_before) / (t1 - t0), 2) if (t1 - t0) > 0 else 0,
            "H_tip_start": tip_before,
            "H_tip_end": tip_after,
        }

    async def get_tip_height(self) -> int:
        try:
            info = await self._rpc_call("get_tip_info")
            self._tip = info.get("height", self._tip)
        except Exception:
            pass
        return self._tip

    async def get_peak_resources(self) -> tuple[float, float]:
        return (self._peak_rss, self._peak_cpu)

    async def shutdown(self) -> None:
        await self._client.aclose()
        log.info("New wallet shut down")

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    async def _cli(self, *args: str) -> str:
        """Run a minotari-cli subcommand and return stdout."""
        cmd = [self._cli_path, "--network", self._cfg.esmeralda_network,
               "--base-node", self._cfg.base_node_url]
        if self._data_dir:
            cmd += ["--data-dir", self._data_dir]
        cmd.extend(str(a) for a in args if a)

        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await asyncio.wait_for(
            proc.communicate(), timeout=120.0
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"minotari-cli {' '.join(args[:3])} failed (rc={proc.returncode}): "
                f"{stderr.decode(errors='replace')[:200]}"
            )
        return stdout.decode(errors="replace")

    async def _rpc_call(self, method: str, params: Any = None) -> dict:
        """Call the base node JSON-RPC endpoint."""
        payload = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params or {},
        }
        resp = await self._client.post(self._cfg.base_node_url, json=payload)
        resp.raise_for_status()
        return resp.json().get("result", {})
