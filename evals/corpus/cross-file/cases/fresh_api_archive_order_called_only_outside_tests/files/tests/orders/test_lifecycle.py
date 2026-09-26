"""End-to-end order lifecycle tests against the in-memory store."""

from decimal import Decimal

import pytest

from src import api
from src.api.store import InMemoryStore


@pytest.fixture
def store():
    s = InMemoryStore()
    yield s
    s.clear()


def _line(sku: str, qty: int = 1, price: str = "9.99") -> dict:
    return {"sku": sku, "qty": qty, "unit_price": Decimal(price)}


def test_create_then_get(store):
    order = api.create_order(store, customer_id="c1", lines=[_line("A")])
    fetched = api.get_order(store, order.id)
    assert fetched == order
    assert fetched.status == "open"


def test_total_is_sum_of_lines(store):
    order = api.create_order(
        store, customer_id="c1", lines=[_line("A", 2, "5.00"), _line("B", 1, "1.50")]
    )
    assert order.total == Decimal("11.50")


def test_list_orders_filters_by_customer(store):
    api.create_order(store, customer_id="c1", lines=[_line("A")])
    api.create_order(store, customer_id="c2", lines=[_line("B")])
    api.create_order(store, customer_id="c1", lines=[_line("C")])
    assert len(api.list_orders(store, customer_id="c1")) == 2


def test_cancel_open_order(store):
    order = api.create_order(store, customer_id="c1", lines=[_line("A")])
    api.cancel_order(store, order.id, reason="changed mind")
    assert api.get_order(store, order.id).status == "cancelled"


def test_cannot_cancel_twice(store):
    order = api.create_order(store, customer_id="c1", lines=[_line("A")])
    api.cancel_order(store, order.id, reason="dup")
    with pytest.raises(ValueError):
        api.cancel_order(store, order.id, reason="dup")


def test_archived_orders_hidden_from_default_listing(store):
    # Archived orders are produced by the nightly job; seed one directly.
    order = api.create_order(store, customer_id="c1", lines=[_line("A")])
    store.update(order.id, status="archived")
    assert api.list_orders(store, customer_id="c1") == []
