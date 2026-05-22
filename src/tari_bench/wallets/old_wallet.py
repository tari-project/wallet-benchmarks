"""
Old wallet implementation — minotari_console_wallet via gRPC.

Manages the wallet process lifecycle (spawn, wait, interact, teardown)
and communicates via gRPC for scanning, balance, and transaction operations.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import signal
import time
from pathlib import Path
from typing import Any

from .base import WalletBase, UtxoInfo, TxResult

log = logging.getLogger(__name__)


class OldWallet(WalletBase):
    """Wraps minotari_console_wallet subprocess + gRPC."""

    GRPC_DEFAULT_PORT = 18142

    def __init__(self, config: Any, grpc_port: int | None = None) -> None:
        self._cfg = config
        self._port = grpc_port or self.GRPC_DEFAULT_PORT
        self._process: asyncio.subprocess.Process | None = None
        self._data_dir: str = ""
        self._peak_rss: float = 0.0
        self._peak_cpu: float = 0.0
        self._tip: int = 0

    def mode_name(self) -> str:
        return "old"

    async def spawn(self, data_dir: str, seed_words: str = "") -> None:
        self._data_dir = data_dir
        data_path = Path(data_dir)
        data_path.mkdir(parents=True, exist_ok=True)

        cmd = [
            self._cfg.console_wallet_bin,
            "--network", self._cfg.esmeralda_network,
            "--base-node", self._cfg.base_node_url,
            "--password", "bench",
            "--grpc-port", str(self._port),
            "--data-dir", str(data_path),
        ]
        if seed_words:
            cmd += ["--seed-words", seed_words]

        log.info("Spawning old wallet: %s", " ".join(cmd))
        self._process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

    async def wait_ready(self, timeout_s: float = 120.0) -> bool:
        start = time.monotonic()
        while time.monotonic() - start < timeout_s:
            try:
                bal = await self.get_balance()
                _ = bal  # if this succeeds, the wallet is responsive
                return True
            except Exception:
                await asyncio.sleep(2.0)
        return False

    async def get_balance(self) -> int:
        """Query gRPC for spendable balance. (Simplified — real impl uses gRPC stubs.)"""
        # Placeholder: actual gRPC call via minotari_wallet_grpc
        # from minotari_wallet_grpc import wallet_client
        # async with wallet_client(f"127.0.0.1:{self._port}") as c:
        #     resp = await c.get_balance()
        #     return resp.available_balance
        raise NotImplementedError(
            "OldWallet gRPC client requires minotari_wallet_grpc Python bindings.\n"
            "Generate stubs: python -m grpc_tools.protoc -Iproto --python_out=. --pyi_out=. --grpc_python_out=. wallet.proto"
        )

    async def get_utxos(self) -> list[UtxoInfo]:
        raise NotImplementedError("gRPC stubs required")

    async def create_and_broadcast(
        self, outputs: list[tuple[str, int]], fee: int | None = None
    ) -> TxResult:
        raise NotImplementedError("gRPC stubs required")

    async def wait_for_confirmation(
        self, tx_id: str, depth: int = 3, timeout_s: float = 300.0
    ) -> bool:
        raise NotImplementedError("gRPC stubs required")

    async def scan(self, from_genesis: bool = False) -> dict:
        raise NotImplementedError("gRPC stubs required")

    async def get_tip_height(self) -> int:
        return self._tip

    async def get_peak_resources(self) -> tuple[float, float]:
        if self._process and self._process.returncode is None:
            try:
                import psutil
                p = psutil.Process(self._process.pid)
                rss = p.memory_info().rss / (1024 * 1024)
                cpu = p.cpu_percent(interval=0.1)
                self._peak_rss = max(self._peak_rss, rss)
                self._peak_cpu = max(self._peak_cpu, cpu)
            except Exception:
                pass
        return (self._peak_rss, self._peak_cpu)

    async def shutdown(self) -> None:
        if self._process and self._process.returncode is None:
            self._process.send_signal(signal.SIGTERM)
            try:
                await asyncio.wait_for(self._process.wait(), timeout=10)
            except asyncio.TimeoutError:
                self._process.kill()
                await self._process.wait()
            log.info("Old wallet shut down (pid=%d)", self._process.pid)
