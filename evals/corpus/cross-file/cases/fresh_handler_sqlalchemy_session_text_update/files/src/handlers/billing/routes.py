from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from sqlalchemy import text

from src import repository
from src.db import SessionLocal

router = APIRouter(prefix="/billing")


class PaymentIn(BaseModel):
    amount: str


class PlanChangeIn(BaseModel):
    plan: str


@router.get("/subscriptions/{sub_id}")
def get_subscription(sub_id: int):
    sub = repository.get_subscription(sub_id)
    if sub is None:
        raise HTTPException(404)
    return {"id": sub.id, "plan": sub.plan, "cancelled_at": sub.cancelled_at}


@router.get("/subscriptions/{sub_id}/invoices")
def invoices(sub_id: int):
    return [
        {"id": i.id, "total": str(i.total), "status": i.status}
        for i in repository.list_invoices(sub_id)
    ]


@router.post("/subscriptions/{sub_id}/cancel")
def cancel(sub_id: int):
    if repository.get_subscription(sub_id) is None:
        raise HTTPException(404)
    repository.cancel_subscription(sub_id, at=datetime.now(timezone.utc))
    return {"ok": True}


@router.post("/subscriptions/{sub_id}/plan")
def change_plan(sub_id: int, body: PlanChangeIn):
    if body.plan not in {"basic", "pro", "enterprise"}:
        raise HTTPException(422, "unknown plan")
    # Quick fix for the plan-migration ticket; move into repository later.
    with SessionLocal.begin() as session:
        session.execute(
            text("UPDATE subscriptions SET plan = :plan WHERE id = :id"),
            {"plan": body.plan, "id": sub_id},
        )
    return {"ok": True, "plan": body.plan}


@router.post("/invoices/{invoice_id}/payments")
def pay(invoice_id: int, body: PaymentIn):
    try:
        amount = Decimal(body.amount)
    except InvalidOperation:
        raise HTTPException(422, "amount must be a decimal string")
    if amount <= 0:
        raise HTTPException(422, "amount must be positive")
    repository.record_payment(invoice_id, amount)
    return {"ok": True}
