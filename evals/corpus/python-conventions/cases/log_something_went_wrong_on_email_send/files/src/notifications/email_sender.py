"""Send transactional email through the SMTP relay."""

from __future__ import annotations

import logging
import smtplib
from dataclasses import dataclass
from email.message import EmailMessage

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class OutboundEmail:
    message_id: str
    to_address: str
    subject: str
    body: str


def _build(email: OutboundEmail, sender: str) -> EmailMessage:
    msg = EmailMessage()
    msg["From"] = sender
    msg["To"] = email.to_address
    msg["Subject"] = email.subject
    msg["Message-ID"] = f"<{email.message_id}@mail.example.com>"
    msg.set_content(email.body)
    return msg


def send(smtp: smtplib.SMTP, email: OutboundEmail, sender: str) -> bool:
    """Send ``email``; return True if the relay accepted it, False otherwise."""
    try:
        smtp.send_message(_build(email, sender))
    except smtplib.SMTPException:
        logger.exception("Something went wrong")
        return False
    logger.info("sent %s to %s", email.message_id, email.to_address)
    return True
