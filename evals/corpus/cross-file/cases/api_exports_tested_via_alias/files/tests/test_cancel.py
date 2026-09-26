from src.api import cancel_order as cancel
from src.api import create_order as place


def test_cancel_marks_the_order_cancelled(db):
    order = place(db, customer_id=7, total_cents=1200)
    cancel(db, order.id, reason="duplicate")
    assert db.get_order(order.id).status == "cancelled"
