"""In-memory stock ledger used by the warehouse picker."""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field


@dataclass
class StockLedger:
    """Tracks on-hand quantity per SKU."""

    on_hand: dict[str, int] = field(default_factory=lambda: defaultdict(int))
    reserved: dict[str, int] = field(default_factory=lambda: defaultdict(int))

    def receive(self, sku: str, quantity: int) -> None:
        """Add ``quantity`` units of ``sku`` to on-hand stock.

        Raises:
            ValueError: if ``quantity`` is not positive.
        """
        if quantity <= 0:
            raise ValueError(f"receive quantity for {sku} must be positive, got {quantity}")
        self.on_hand[sku] += quantity

    def reserve(self, sku: str, quantity: int) -> None:
        """Reserve ``quantity`` units of ``sku`` for a pick list.

        Raises:
            ValueError: if fewer than ``quantity`` units are available.
        """
        available = self.available(sku)
        if available < quantity:
            raise KeyError(f"{sku}: only {available} available, {quantity} requested")
        self.reserved[sku] += quantity

    def release(self, sku: str, quantity: int) -> None:
        """Return ``quantity`` previously reserved units of ``sku`` to stock.

        Releasing more than is reserved clamps the reservation at zero.
        """
        self.reserved[sku] = max(0, self.reserved[sku] - quantity)

    def available(self, sku: str) -> int:
        """Return on-hand minus reserved units for ``sku`` (0 for an unknown SKU)."""
        return self.on_hand.get(sku, 0) - self.reserved.get(sku, 0)
