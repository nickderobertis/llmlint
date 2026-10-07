#!/usr/bin/env bash
# CI's gate routing and the release's sweep verdict — one file, so what counts as
# "the full sweep" is defined once for the job that runs it and the job that
# trusts it (AGENTS.md, "Commits, releases, and merging", records the placement):
#
#   ci-gate.sh tier      Which tier this CI run owes. Reads GITHUB_EVENT_NAME and
#                        the payload at GITHUB_EVENT_PATH; writes tier=affected|all,
#                        base=<sha> (affected only) and head=<sha> (a release PR's
#                        head, to check out so the sweep tests exactly that tree)
#                        to GITHUB_OUTPUT, or to stdout outside Actions.
#                          * pull_request from a release-plz branch of this repo
#                            -> all: release-plz batches merges behind that PR, so
#                            the broader tier runs once, at release-prep;
#                          * any other pull_request -> affected, base = the merge
#                            base with origin/<the PR's base branch>;
#                          * push (merge-to-main) -> affected, base = event.before;
#                          * workflow_dispatch -> all (a manual sweep: the
#                            recovery when a release finds no verdict).
#   ci-gate.sh verdict   May the release ship? Asks GitHub for the sweep runs of
#                        ci.yml whose tested commit has the released commit's tree
#                        and exits 0 only when the newest one's gate and cross jobs
#                        all concluded success. A red, missing, or different-tree
#                        verdict stops the release (exit 1). Reads REPO and SHA
#                        (falling back to GITHUB_REPOSITORY / GITHUB_SHA),
#                        CI_WAIT_ATTEMPTS (default 20) and CI_WAIT_DELAY (default
#                        30 seconds) for a sweep still running; needs `gh` (with
#                        GH_TOKEN) and `jq`.
#
# Exit status: 0 decided (tier) / may ship (verdict); 1 refused or undecidable,
# with the cause and the next action on stderr; 2 a usage error.
set -euo pipefail

# release-plz names its release-PR branches `release-plz-<timestamp>`
# (release-plz.yml's auto-merge step selects them by the same prefix; the
# ci-workflows tests hold the two equal).
readonly RELEASE_PR_PREFIX="release-plz-"
readonly CI_WORKFLOW="ci.yml"
readonly SWEEP_JOBS='["gate", "cross (macos-latest)", "cross (windows-latest)"]'

usage() {
  printf 'ci-gate: %s\n' "$1" >&2
  printf '  usage: .github/scripts/ci-gate.sh tier | verdict\n' >&2
  exit 2
}

refuse() {
  printf '::error::ci-gate: %s\n' "$1" >&2
  printf '  next: %s\n' "$2" >&2
  exit 1
}

emit() {
  local lines="$1" why="$2"
  if [ -z "${GITHUB_OUTPUT:-}" ]; then
    printf '%s' "$lines"
  elif ! printf '%s' "$lines" >>"$GITHUB_OUTPUT"; then
    refuse "could not write to GITHUB_OUTPUT ($GITHUB_OUTPUT)" "re-run the job; the runner provides a writable GITHUB_OUTPUT"
  fi
  printf 'ci-gate: %s\n' "$why" >&2
}

is_sha() { [[ "$1" =~ ^[0-9a-f]{40}$ ]] && [[ ! "$1" =~ ^0+$ ]]; }

route() {
  local event="${GITHUB_EVENT_NAME:-}" payload_path="${GITHUB_EVENT_PATH:-}" payload
  [ -n "$event" ] || usage "GITHUB_EVENT_NAME is empty; run this inside a GitHub Actions job (or set it and GITHUB_EVENT_PATH)"
  if [ -n "$payload_path" ]; then
    payload="$(jq -c 'if type == "object" then . else error("not a JSON object") end' "$payload_path" 2>&1)" ||
      refuse "the event payload at GITHUB_EVENT_PATH ($payload_path) is unreadable: $payload" "point GITHUB_EVENT_PATH at the event JSON the runner provides"
  else
    payload='{}'
  fi
  # llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] GitHub owns the event payload shape and its webhook docs are the only authority, which no gate run can reach offline; every field read is one GitHub documents for pull_request and push events, and a payload missing one fails safe: no usable base ref or head sha is refused, an unrecognised release PR gets the affected tier (so the release's verdict check finds no sweep and stops), and a push without event.before falls back to HEAD~1
  case "$event" in
    pull_request)
      local head_ref head_repo base_repo head_sha base_ref base
      head_ref="$(jq -r '.pull_request.head.ref // ""' <<<"$payload")"
      head_repo="$(jq -r '.pull_request.head.repo.full_name // ""' <<<"$payload")"
      base_repo="$(jq -r '.pull_request.base.repo.full_name // ""' <<<"$payload")"
      if [[ "$head_ref" == "$RELEASE_PR_PREFIX"* ]] && [ -n "$head_repo" ] && [ "$head_repo" = "$base_repo" ]; then
        head_sha="$(jq -r '.pull_request.head.sha // ""' <<<"$payload")"
        is_sha "$head_sha" || refuse "release PR payload has no usable head sha ('$head_sha')" "re-run CI on the release PR"
        emit $'tier=all\nbase=\nhead='"$head_sha"$'\n' "all — release PR ($head_ref): the full sweep at release-prep, on its head $head_sha"
        return
      fi
      base_ref="$(jq -r '.pull_request.base.ref // ""' <<<"$payload")"
      if [[ ! "$base_ref" =~ ^[A-Za-z0-9._/-]+$ ]] || [[ "$base_ref" == *..* ]] || [[ "$base_ref" == -* ]]; then
        refuse "pull_request payload has no usable base ref ('$base_ref')" "target a branch named with letters, digits and . _ / - only; nothing was run"
      fi
      base="$(git merge-base "origin/$base_ref" HEAD 2>&1)" ||
        refuse "no merge base with origin/$base_ref: $base" "check out with fetch-depth: 0 so the base branch and its history are present"
      emit "tier=affected"$'\n'"base=$base"$'\nhead=\n' "affected — pull request: merge base with origin/$base_ref ($base)"
      ;;
    push)
      local before
      before="$(jq -r '.before // ""' <<<"$payload")"
      if is_sha "$before" && git rev-parse --verify --quiet "$before^{commit}" >/dev/null; then
        emit "tier=affected"$'\n'"base=$before"$'\nhead=\n' "affected — push: since the previous tip ($before)"
      else
        before="$(git rev-parse HEAD~1 2>&1)" ||
          refuse "push without a usable event.before, and no HEAD~1: $before" "check out with fetch-depth: 0"
        emit "tier=affected"$'\n'"base=$before"$'\nhead=\n' "affected — push: since HEAD~1 (event.before unavailable)"
      fi
      ;;
    # llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    workflow_dispatch)
      emit $'tier=all\nbase=\nhead=\n' "all — workflow_dispatch: a manual full sweep of the dispatched commit"
      ;;
    *)
      refuse "no tier is defined for the '$event' event" "trigger ci.yml on pull_request, push or workflow_dispatch only"
      ;;
  esac
}

gh_api() {
  local out
  if ! out="$(gh api "$1" 2>&1)"; then
    refuse "could not read $1 from GitHub: $out" "restore the job's actions:read permission and GH_TOKEN, then re-run the release"
  fi
  printf '%s' "$out"
}

verdict() {
  local repo="${REPO:-${GITHUB_REPOSITORY:-}}" sha="${SHA:-${GITHUB_SHA:-}}"
  local attempts="${CI_WAIT_ATTEMPTS:-20}" delay="${CI_WAIT_DELAY:-30}"
  [[ "$repo" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || usage "REPO '$repo' is not an owner/name repository"
  is_sha "$sha" || usage "SHA '$sha' is not a full 40-character commit sha"
  # Plain decimal only: a leading zero would be read as octal by the loop below.
  [[ "$attempts" =~ ^[1-9][0-9]{0,3}$ ]] || usage "CI_WAIT_ATTEMPTS '$attempts' is not a whole number of polls (1-9999, no leading zero)"
  [[ "$delay" =~ ^(0|[1-9][0-9]{0,3})$ ]] || usage "CI_WAIT_DELAY '$delay' is not a whole number of seconds (0-9999, no leading zero)"

  local commit tree
  commit="$(gh_api "repos/$repo/commits/$sha")"
  if ! tree="$(jq -r '.commit.tree.sha // ""' <<<"$commit" 2>&1)" || ! is_sha "$tree"; then
    refuse "GitHub returned no tree for $sha (repos/$repo/commits/$sha answered: ${commit:0:200})" "check that $sha is pushed to $repo and the token can read it, then re-run the release"
  fi

  # llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] GitHub owns these response shapes and its API is the only authority, which no gate run can reach offline; every field read is one GitHub's REST docs define for workflow runs and jobs, and an answer missing any of them is refused as unreadable rather than trusted
  local poll runs run run_id run_url run_status jobs state
  for ((poll = 1; poll <= attempts; poll++)); do
    runs="$(
      {
        gh_api "repos/$repo/actions/workflows/$CI_WORKFLOW/runs?event=pull_request&per_page=100"
        gh_api "repos/$repo/actions/workflows/$CI_WORKFLOW/runs?event=workflow_dispatch&per_page=100"
      } | jq -cs --arg repo "$repo" --arg prefix "$RELEASE_PR_PREFIX" '
        [ .[] | (.workflow_runs // error("the answer has no workflow_runs")) | .[]
          | select(.event == "workflow_dispatch"
                   or (.event == "pull_request"
                       and ((.head_branch // "") | startswith($prefix))
                       and .head_repository.full_name == $repo)) ]
        | sort_by(.created_at, .id)' 2>&1
    )" || refuse "GitHub's answer for the $CI_WORKFLOW runs was unreadable: $runs" "inspect the API response, then re-run the release"
    run="$(jq -c --arg tree "$tree" '[ .[] | select(.head_commit.tree_id == $tree) ] | last // empty' <<<"$runs")"
    if [ -z "$run" ]; then
      local other
      other="$(jq -r 'last // empty | "run \(.id) swept tree \(.head_commit.tree_id) (commit \(.head_sha))"' <<<"$runs")"
      refuse "no full-sweep CI run tested the released tree $tree (commit $sha)${other:+; the latest sweep: $other}" \
        "run the CI workflow by hand on $sha (workflow_dispatch sweeps it), then re-run this release"
    fi
    run_id="$(jq -r '.id' <<<"$run")"
    [[ "$run_id" =~ ^[1-9][0-9]{0,19}$ ]] ||
      refuse "GitHub's answer names a CI run whose id is not a positive integer ('$run_id')" "inspect the API response, then re-run the release"
    run_url="$(jq -r '.html_url // ""' <<<"$run")"
    run_status="$(jq -r '.status // ""' <<<"$run")"
    jobs="$(gh_api "repos/$repo/actions/runs/$run_id/jobs?filter=latest&per_page=100")"
    state="$(jq -r --argjson want "$SWEEP_JOBS" '
      (.jobs // error("the answer has no jobs")) as $jobs
      | [ $want[] as $name | ($jobs | map(select(.name == $name)) | last)
          | if . == null then "pending \($name) (not started)"
            elif .status != "completed" then "pending \($name) (\(.status))"
            elif .conclusion == "success" then "ok \($name)"
            else "red \($name) (\(.conclusion))" end ]
      | (map(select(startswith("red "))) | first)
        // (map(select(startswith("pending "))) | first)
        // "green"' <<<"$jobs" 2>&1)" ||
      refuse "the jobs of CI run $run_id were unreadable: $state" "inspect the API response, then re-run the release"
    case "$state" in
      green)
        printf 'ci-gate: CI run %s (%s) swept tree %s and its gate and cross jobs concluded success; releasing %s.\n' \
          "$run_id" "$run_url" "$tree" "$sha"
        exit 0
        ;;
      red\ *)
        refuse "the full sweep of tree $tree failed: CI run $run_id job ${state#red } ($run_url)" "fix the commit and release the fix; nothing was shipped"
        ;;
    esac
    # A finished run whose job never reported will not report later.
    if [ "$run_status" = completed ]; then
      refuse "CI run $run_id finished without a verdict from job ${state#pending } ($run_url)" "re-run that job (or the CI workflow on $sha by hand), then re-run this release"
    fi
    # Narrated once, on the first wait: the later polls add nothing a reader needs.
    if [ "$poll" -eq 1 ]; then
      printf 'ci-gate: CI run %s: %s; waiting (up to %s polls, %ss apart)\n' "$run_id" "${state#pending }" "$attempts" "$delay" >&2
    fi
    [ "$poll" -lt "$attempts" ] && sleep "$delay"
  done
  # llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
  refuse "the full sweep of tree $tree (CI run $run_id) did not finish within $attempts polls" "wait for CI run $run_id to settle ($run_url), then re-run this release"
}

[ $# -eq 1 ] || usage "expected exactly one step"
case "$1" in
  tier) route ;;
  verdict) verdict ;;
  *) usage "unknown step '$1'" ;;
esac
