"""
Abstract base class for wallet implementations.

Defines the contract every wallet mode must fulfil.
"""

from __future__ import annotations

import abc
from dataclasses import dataclass
from typing import Protocol


class WalletMode(Protocol):
    """Protocol for wallet mode identifiers."""
    name: str


@dataclass
class UtxoInfo:
    """Represents a spendable UTXO."""
    commitment: bytes
    value: int
    height: int
    spent: bool = False


@dataclass
class TxResult:
    """Outcome of building and broadcasting a single transaction."""
    tx_id: str = ""
    success: bool = False
    fee_paid: int = 0
    error: str | None = None
    construction_ns: int = 0
    confirmation_ns: int = 0
    inputs: int = 0
    outputs: int = 0


class WalletBase(abc.ABC):
    """Abstract wallet interface."""

    @abc.abstractmethod
    def mode_name(self) -> str:
        """Return 'old', 'new', or 'payment'."""
        ...

    @abc.abstractmethod
    async def spawn(self, data_dir: str, seed_words: str = "") -> None:
        """Start the wallet process / connect to library."""
        ...

    @abc.abstractmethod
    async def wait_ready(self, timeout_s: float = 120.0) -> bool:
        """Block until wallet is synced and ready. Return True on success."""
        ...

    @abc.abstractmethod
    async def get_balance(self) -> int:
        """Return spendable balance in tXTM."""
        ...

    @abc.abstractmethod
    async def get_utxos(self) -> list[UtxoInfo]:
        """List all spendable UTXOs."""
        ...

    @abc.abstractmethod
    async def create_and_broadcast(
        self,
        outputs: list[tuple[str, int]],  # [(address, amount), ...]
        fee: int | None = None,
    ) -> TxResult:
        """Build, sign, and broadcast a transaction. Return the result."""
        ...

    @abc.abstractmethod
    async def wait_for_confirmation(
        self, tx_id: str, depth: int = 3, timeout_s: float = 300.0
    ) -> bool:
        """Wait until a tx reaches the required confirmation depth."""
        ...

    @abc.abstractmethod
    async def scan(self, from_genesis: bool = False) -> dict:
        """Trigger a wallet scan. Return scan metrics dict."""
        ...

    @abc.abstractmethod
    async def shutdown(self) -> None:
        """Stop the wallet and clean up resources."""
        ...

    @abc.abstractmethod
    async def get_tip_height(self) -> int:
        """Return the current chain tip height."""
        ...

    @abc.abstractmethod
    async def get_peak_resources(self) -> tuple[float, float]:
        """Return (peak_rss_mb, peak_cpu_pct) since last call."""
        ...
