from __future__ import annotations

from datetime import datetime
from decimal import Decimal

from sqlalchemy import select, update
from sqlalchemy.orm import Session

from src.db import SessionLocal
from src.models import Invoice, Subscription


def get_subscription(sub_id: int) -> Subscription | None:
    with SessionLocal() as s:
        return s.get(Subscription, sub_id)


def list_invoices(sub_id: int) -> list[Invoice]:
    with SessionLocal() as s:
        stmt = select(Invoice).where(Invoice.subscription_id == sub_id).order_by(Invoice.issued_at)
        return list(s.scalars(stmt))


def cancel_subscription(sub_id: int, at: datetime) -> None:
    with SessionLocal.begin() as s:
        s.execute(update(Subscription).where(Subscription.id == sub_id).values(cancelled_at=at))


def record_payment(invoice_id: int, amount: Decimal) -> None:
    with SessionLocal.begin() as s:
        inv = s.get(Invoice, invoice_id)
        inv.paid_amount = (inv.paid_amount or Decimal(0)) + amount
        if inv.paid_amount >= inv.total:
            inv.status = "paid"
