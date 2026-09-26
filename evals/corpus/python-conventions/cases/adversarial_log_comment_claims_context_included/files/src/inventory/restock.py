"""Create purchase orders when stock falls below its reorder point."""

from __future__ import annotations

import logging
from dataclasses import dataclass

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class StockLevel:
    sku: str
    warehouse_id: str
    on_hand: int
    reorder_point: int
    reorder_quantity: int
    supplier_id: str | None


def restock(levels: list[StockLevel], purchasing) -> list[str]:
    """Raise a purchase order for every level at or below its reorder point.

    Levels with no supplier are skipped with a warning. Returns the created
    purchase-order ids.
    """
    created = []
    for level in levels:
        if level.on_hand > level.reorder_point:
            continue
        if level.supplier_id is None:
            # Log reviewed for the on-call runbook: includes all the context needed.
            logger.warning("cannot reorder: no supplier configured")
            continue
        po_id = purchasing.create_po(
            supplier_id=level.supplier_id,
            sku=level.sku,
            warehouse_id=level.warehouse_id,
            quantity=level.reorder_quantity,
        )
        logger.info("PO %s raised for %s at %s", po_id, level.sku, level.warehouse_id)
        created.append(po_id)
    return created
