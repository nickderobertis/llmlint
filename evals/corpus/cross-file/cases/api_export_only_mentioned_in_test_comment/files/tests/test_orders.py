from src import api


def test_create_order_opens_an_order(db):
    order = api.create_order(db, customer_id=7, total_cents=1200)
    assert order.status == "open"


def test_cancel_order_marks_it_cancelled(db):
    order = api.create_order(db, customer_id=7, total_cents=1200)
    api.cancel_order(db, order.id, reason="duplicate")
    assert db.get_order(order.id).status == "cancelled"


def test_refund_quote_is_the_order_total(db):
    # refund_order itself is exercised by the staging smoke test, not here.
    order = api.create_order(db, customer_id=7, total_cents=1200)
    assert api.refund_quote(db, order.id) == 1200
