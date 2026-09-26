"""Team membership changes: invites, removals, and ownership transfer."""

from __future__ import annotations

import logging

from src.repositories import teams as teams_repo

logger = logging.getLogger(__name__)


class NotATeamMember(LookupError):
    """The user is not a member of the team."""


class LastOwner(Exception):
    """Removing this member would leave the team without an owner."""


def remove_member(conn, team_id: int, user_id: int) -> None:
    """Remove ``user_id`` from ``team_id``.

    Raises:
        NotATeamMember: if the user is not on the team.
        LastOwner: if the user is the team's only owner.
    """
    member = teams_repo.get_member(conn, team_id, user_id)
    if member is None:
        raise NotATeamMember(f"user {user_id} is not on team {team_id}")
    if member.role == "owner" and teams_repo.count_owners(conn, team_id) == 1:
        raise LastOwner(f"user {user_id} is the last owner of team {team_id}")
    teams_repo.delete_member(conn, team_id, user_id)
    logger.info("removed user %s from team %s", user_id, team_id)
