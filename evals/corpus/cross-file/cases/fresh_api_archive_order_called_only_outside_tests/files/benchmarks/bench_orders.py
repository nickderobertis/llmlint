"""Rough throughput benchmark: python -m benchmarks.bench_orders"""

import time
from decimal import Decimal

from src import api
from src.api.store import InMemoryStore


def main(n: int = 10_000) -> None:
    store = InMemoryStore()
    ids = [
        api.create_order(
            store, customer_id=f"c{i % 50}",
            lines=[{"sku": "A", "qty": 1, "unit_price": Decimal("1")}],
        ).id
        for i in range(n)
    ]
    start = time.perf_counter()
    for order_id in ids:
        api.archive_order(store, order_id)
    elapsed = time.perf_counter() - start
    print(f"archived {n} orders in {elapsed:.3f}s")


if __name__ == "__main__":
    main()
