"""Email finalized invoices to customers."""

from __future__ import annotations

import functools
import logging
from collections.abc import Callable
from typing import ParamSpec, TypeVar

logger = logging.getLogger(__name__)

P = ParamSpec("P")
R = TypeVar("R")


def best_effort(fn: Callable[P, R]) -> Callable[P, R | None]:
    """Decorator for non-critical side effects."""

    @functools.wraps(fn)
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> R | None:
        try:
            return fn(*args, **kwargs)
        except Exception:
            return None

    return wrapper


def _render(invoice) -> str:
    lines = [f"Invoice {invoice.number}", ""]
    lines += [f"{item.description:<40} {item.amount:>10.2f}" for item in invoice.items]
    lines += ["", f"{'Total':<40} {invoice.total:>10.2f}"]
    return "\n".join(lines)


@best_effort
def send_invoice(mailer, invoice) -> str:
    """Email ``invoice`` to its billing contact and return the provider message id."""
    return mailer.send(
        to=invoice.billing_email,
        subject=f"Invoice {invoice.number}",
        body=_render(invoice),
    )


def finalize_and_send(store, mailer, invoice) -> None:
    """Mark ``invoice`` finalized, then email it.

    The invoice is finalized even if the email fails.
    """
    store.finalize(invoice.invoice_id)
    message_id = send_invoice(mailer, invoice)
    store.record_delivery(invoice.invoice_id, message_id)
