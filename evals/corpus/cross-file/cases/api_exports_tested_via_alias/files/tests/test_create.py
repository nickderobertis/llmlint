from src import api


def test_create_order_opens_an_order(db):
    order = api.create_order(db, customer_id=7, total_cents=1200)
    assert order.status == "open"
