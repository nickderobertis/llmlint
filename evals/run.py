#!/usr/bin/env python3
"""Accuracy + timing eval for llmlint's judge backends.

Drives the REAL built `llmlint` binary over a committed, labeled corpus
(`evals/corpus/`) and scores what it reports against the planted ground truth:
violation recall/precision/F1, line-localization hit rate, run-to-run stability,
wall-clock latency, and (for real harness runs) tokens + dollars. One *arm* is one
way of judging; comparing arms on the same corpus is the before/after
measurement. See `evals/AGENTS.md` for the corpus format and metric definitions.

Arms:
  harness  real oneharness + a real harness (paid). The pre-Jev baseline.
  jev      every rule judged by TypeSafe's Jev (`LLMLINT_JUDGE=jev`; needs
           TYPESAFE_API_KEY; paid, fractions of a cent). No oneharness.
  cascade  Jev first, uncertain rule/files re-judged by the harness
           (`LLMLINT_JUDGE=cascade`; needs both; the band is `jev.escalate_*`).
  oracle   the mock-oneharness fixture answering from the labels themselves: a
           perfect judge. A free, hermetic self-test of this runner — every
           metric must come out perfect, or the plumbing/scoring is broken.
  blind    the mock-oneharness fixture passing every rule: a judge that never
           flags anything. Free; must score recall 0 (proves misses are counted).

Stdlib only (Python >= 3.11 for `tomllib`).
"""

from __future__ import annotations

import argparse
import hashlib
import concurrent.futures
import datetime as dt
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CORPUS = REPO / "evals" / "corpus"
RESULTS = REPO / "evals" / "results"

# git's repository-selection variables outrank `-C` (see `AMBIENT_REPOSITORY_VARS`
# in src/io/diff.rs); a scratch repo built under an inherited GIT_DIR (e.g. from a
# hook) would silently be the outer repository. Clear them for every git spawn.
GIT_AMBIENT = (
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
)

ARMS = ("harness", "jev", "cascade", "oracle", "blind")


# --- corpus ------------------------------------------------------------------


@dataclass(frozen=True)
class Expect:
    """Ground truth for one rule in one case."""

    rule: str
    holds: bool
    # 1-based lines in `file` where the planted violation sits (any one reported
    # is a hit). Empty for a holding rule, or a violation with no single line.
    lines: tuple[int, ...] = ()
    file: str | None = None


@dataclass(frozen=True)
class Case:
    suite: str
    name: str
    dir: Path
    config: Path
    mode: str  # "file" | "diff"
    description: str
    expects: tuple[Expect, ...]
    # An explicit `split = "test"` in case.toml pins a case held out (e.g. cases
    # written after a technique was tuned, so they were never seen while tuning).
    pinned_split: str | None = None

    @property
    def id(self) -> str:
        return f"{self.suite}/{self.name}"

    @property
    def split(self) -> str:
        """`test` (held out) or `dev`, fixed by a stable hash of the case id so a
        case never changes sides and a new case lands on one automatically.
        Tune judge techniques on `dev` only; `test` is the number to report."""
        if self.pinned_split:
            return self.pinned_split
        return "test" if int(hashlib.sha256(self.id.encode()).hexdigest(), 16) % 3 == 0 else "dev"


def load_case(suite_dir: Path, case_dir: Path) -> Case:
    meta = tomllib.loads((case_dir / "case.toml").read_text())
    mode = meta.get("mode", "file")
    if meta.get("split") not in (None, "dev", "test"):
        raise ValueError(f"{case_dir}: split must be 'dev' or 'test', got {meta.get('split')!r}")
    if mode not in ("file", "diff"):
        raise ValueError(f"{case_dir}: mode must be 'file' or 'diff', got {mode!r}")
    if not (case_dir / "files").is_dir():
        raise ValueError(f"{case_dir}: missing files/ (the tree under test)")
    if mode == "diff" and not (case_dir / "base").is_dir():
        raise ValueError(f"{case_dir}: diff mode needs base/ (the pre-change tree)")
    expects = []
    for e in meta.get("expect", []):
        holds = e["holds"]
        lines = tuple(int(n) for n in e.get("lines", []))
        if holds and lines:
            raise ValueError(f"{case_dir}: rule {e['rule']} holds but lists violation lines")
        if not holds and lines and not e.get("file"):
            raise ValueError(f"{case_dir}: rule {e['rule']} lists lines without a file")
        if e.get("file"):
            target = case_dir / "files" / e["file"]
            if not target.is_file():
                raise ValueError(f"{case_dir}: rule {e['rule']} names missing file {e['file']}")
            n_lines = len(target.read_text().splitlines())
            bad = [n for n in lines if not 1 <= n <= n_lines]
            if bad:
                raise ValueError(
                    f"{case_dir}: rule {e['rule']} cites line(s) {bad} outside {e['file']} (1..{n_lines})"
                )
        expects.append(Expect(e["rule"], holds, lines, e.get("file")))
    if not expects:
        raise ValueError(f"{case_dir}: no [[expect]] entries")
    names = [e.rule for e in expects]
    if len(set(names)) != len(names):
        raise ValueError(f"{case_dir}: a rule is expected twice")
    return Case(
        suite=suite_dir.name,
        name=case_dir.name,
        dir=case_dir,
        config=suite_dir / "llmlint.yml",
        mode=mode,
        description=meta.get("description", ""),
        expects=tuple(expects),
        pinned_split=meta.get("split"),
    )


def load_corpus(root: Path, suites: list[str] | None = None) -> list[Case]:
    cases = []
    for suite_dir in sorted(p for p in root.iterdir() if p.is_dir()):
        if suites and suite_dir.name not in suites:
            continue
        if not (suite_dir / "llmlint.yml").is_file():
            raise ValueError(f"{suite_dir}: missing llmlint.yml (the suite's rules)")
        for case_dir in sorted(p for p in (suite_dir / "cases").iterdir() if p.is_dir()):
            cases.append(load_case(suite_dir, case_dir))
    if suites:
        missing = set(suites) - {c.suite for c in cases}
        if missing:
            raise ValueError(f"unknown suite(s): {', '.join(sorted(missing))}")
    return cases


# --- materialize + run -------------------------------------------------------


def git(cwd: Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in GIT_AMBIENT}
    env.update(
        GIT_AUTHOR_NAME="eval",
        GIT_AUTHOR_EMAIL="eval@example.invalid",
        GIT_COMMITTER_NAME="eval",
        GIT_COMMITTER_EMAIL="eval@example.invalid",
    )
    subprocess.run(
        # A scratch repo must not depend on the user's git config: global commit
        # signing (an agent prompt, a flaky signer under -j) would fail setup.
        ["git", "-C", str(cwd), "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
        env=env,
        check=True,
        capture_output=True,
    )


def materialize(case: Case, dest: Path) -> None:
    """Lay out the tree under test in `dest`. A diff case commits `base/` first,
    then swaps in `files/` so the change is the working-tree diff. The suite
    config is passed by `--config` from outside `dest`, so it is never itself a
    lint target (the config-lint suite lints llmlint configs)."""
    if case.mode == "diff":
        shutil.copytree(case.dir / "base", dest, dirs_exist_ok=True)
        git(dest, "init", "-q")
        git(dest, "add", "-A")
        git(dest, "commit", "-q", "-m", "base")
        for child in dest.iterdir():
            if child.name != ".git":
                shutil.rmtree(child) if child.is_dir() else child.unlink()
        shutil.copytree(case.dir / "files", dest, dirs_exist_ok=True)
    else:
        shutil.copytree(case.dir / "files", dest, dirs_exist_ok=True)


def oracle_verdicts(case: Case) -> dict:
    """The mock-oneharness verdict map a perfect judge would return."""
    out = {}
    for e in case.expects:
        if e.holds:
            out[e.rule] = True
        else:
            out[e.rule] = {
                "holds": False,
                "violations": [
                    {"file": e.file, "line": n, "message": "planted"} for n in e.lines
                ]
                or [{"message": "planted"}],
            }
    return out


@dataclass
class Opts:
    llmlint: Path
    oneharness: Path | None
    mock: Path | None
    model: str | None
    timeout: int


@dataclass
class Run:
    case: str
    rep: int
    exit: int
    seconds: float
    report: dict | None
    stderr: str
    usage: dict = field(default_factory=dict)


USAGE_KEYS = (
    "calls",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "cost_usd",
    "duration_ms",
)


def usage_of(report: dict | None) -> dict:
    """Total what every judge backend consumed, from llmlint's own `usage`
    block (one entry per backend that made a call)."""
    tot = {k: 0 for k in USAGE_KEYS}
    tot["cost_usd"] = 0.0
    for backend in ((report or {}).get("usage") or {}).values():
        for k in USAGE_KEYS:
            tot[k] += backend.get(k) or 0
    return tot


def run_case(case: Case, arm: str, rep: int, opts: Opts) -> Run:
    with tempfile.TemporaryDirectory(prefix="llmlint-eval-") as tmp:
        proj = Path(tmp) / "proj"
        proj.mkdir()
        materialize(case, proj)
        env = {k: v for k, v in os.environ.items() if k not in GIT_AMBIENT}
        cmd = [
            str(opts.llmlint),
            "--cwd", str(proj),
            "--config", str(case.config),
            "--format", "json",
            "--no-history",
            "--progress", "never",
            "--timeout", str(opts.timeout),
        ]
        for e in case.expects:
            cmd += ["--rule", e.rule]
        if case.mode == "diff":
            cmd.append("--diff")
        if arm == "harness":
            cmd += ["--oneharness-bin", str(opts.oneharness)]
            if opts.model:
                cmd += ["--model", opts.model]
        elif arm == "jev":
            env["LLMLINT_JUDGE"] = "jev"
        elif arm == "cascade":
            env["LLMLINT_JUDGE"] = "cascade"
            cmd += ["--oneharness-bin", str(opts.oneharness)]
            if opts.model:
                cmd += ["--model", opts.model]
        else:
            verdicts = Path(tmp) / "verdicts.json"
            verdicts.write_text(json.dumps(oracle_verdicts(case) if arm == "oracle" else {}))
            env["LLMLINT_MOCK_VERDICTS"] = str(verdicts)
            env["LLMLINT_MOCK_STATE"] = str(Path(tmp) / "state")
            cmd += ["--oneharness-bin", str(opts.mock)]
        start = time.monotonic()
        p = subprocess.run(cmd, env=env, capture_output=True, text=True)
        seconds = time.monotonic() - start
        try:
            report = json.loads(p.stdout) if p.stdout.strip() else None
        except json.JSONDecodeError:
            report = None
        return Run(case.id, rep, p.returncode, seconds, report, p.stderr, usage_of(report))


# --- scoring (pure) ----------------------------------------------------------


def norm(path: str | None) -> str | None:
    if path is None:
        return None
    p = path.replace("\\", "/")
    while p.startswith("./"):
        p = p[2:]
    return p


def line_hit(expect: Expect, violations: list[dict], tol: int) -> bool:
    """True when any reported violation lands on (within `tol` of) a planted line,
    in the planted file when both sides name one."""
    for v in violations:
        vf, vl = norm(v.get("file")), v.get("line")
        if vl is None:
            continue
        if expect.file and vf and vf != norm(expect.file):
            continue
        end = v.get("end_line") or vl
        for n in expect.lines:
            if vl - tol <= n <= end + tol:
                return True
    return False


def judge_item(expect: Expect, run: Run, tol: int) -> dict:
    """Score one (case, rule) against one run. `predicted` is True when the rule
    was flagged (outcome fail), False when it was not, None when the run could
    not complete (exit 2 / unparseable / rule missing from the report)."""
    item = {"rule": expect.rule, "expected_violation": not expect.holds}
    rules = {r["name"]: r for r in (run.report or {}).get("rules", [])}
    r = rules.get(expect.rule)
    if run.exit == 2 or r is None or r.get("outcome") in ("skipped", "ignored"):
        item.update(predicted=None, error=True)
        return item
    flagged = r["outcome"] == "fail"
    item.update(predicted=flagged, error=False)
    # A probabilistic judge's raw number (the least-holding file), kept so an
    # escalation band can be swept offline from this result file.
    scores = r.get("scores") or []
    if scores:
        item["p_holds"] = min(s["p_holds"] for s in scores)
    if flagged and not expect.holds and expect.lines:
        item["line_hit"] = line_hit(expect, r.get("violations", []), tol)
    return item


def score_runs(cases: list[Case], runs: list[Run], tol: int) -> list[dict]:
    by_case = {c.id: c for c in cases}
    items = []
    for run in runs:
        case = by_case[run.case]
        for e in case.expects:
            it = judge_item(e, run, tol)
            it.update(case=case.id, suite=case.suite, split=case.split, rep=run.rep)
            items.append(it)
    return items


def ratio(n: int, d: int) -> float | None:
    return None if d == 0 else n / d


def percentile(xs: list[float], q: float) -> float | None:
    if not xs:
        return None
    s = sorted(xs)
    k = (len(s) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def metrics(items: list[dict], runs: list[Run]) -> dict:
    done = [i for i in items if not i["error"]]
    tp = sum(1 for i in done if i["expected_violation"] and i["predicted"])
    fn = sum(1 for i in done if i["expected_violation"] and not i["predicted"])
    fp = sum(1 for i in done if not i["expected_violation"] and i["predicted"])
    tn = sum(1 for i in done if not i["expected_violation"] and not i["predicted"])
    recall, precision = ratio(tp, tp + fn), ratio(tp, tp + fp)
    f1 = (
        None
        if recall is None or precision is None or recall + precision == 0
        else 2 * recall * precision / (recall + precision)
    )
    located = [i for i in done if "line_hit" in i]

    # Stability: an item is unstable when its completed repeats disagree.
    seen: dict[tuple[str, str], set] = {}
    for i in done:
        seen.setdefault((i["case"], i["rule"]), set()).add(i["predicted"])
    unstable = sum(1 for v in seen.values() if len(v) > 1)

    secs = [r.seconds for r in runs]
    cost = sum(r.usage.get("cost_usd", 0.0) for r in runs)
    tokens = sum(
        r.usage.get(k, 0)
        for r in runs
        for k in ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")
    )
    return {
        "items": len(items),
        "errors": len(items) - len(done),
        "tp": tp,
        "fn": fn,
        "fp": fp,
        "tn": tn,
        "recall": recall,
        "precision": precision,
        "f1": f1,
        "accuracy": ratio(tp + tn, len(done)),
        "line_hit_rate": ratio(sum(1 for i in located if i["line_hit"]), len(located)),
        "unstable_items": unstable,
        "invocations": len(runs),
        "latency_p50_s": percentile(secs, 0.5),
        "latency_p95_s": percentile(secs, 0.95),
        "latency_total_s": sum(secs),
        "judge_calls": sum(r.usage.get("calls", 0) for r in runs),
        "tokens": tokens,
        "cost_usd": cost,
    }


def summarize(cases: list[Case], runs: list[Run], tol: int) -> dict:
    items = score_runs(cases, runs, tol)
    suites = sorted({c.suite for c in cases})
    return {
        "overall": metrics(items, runs),
        "suites": {
            s: metrics(
                [i for i in items if i["suite"] == s],
                [r for r in runs if r.case.split("/", 1)[0] == s],
            )
            for s in suites
        },
        "items": items,
    }


# --- report ------------------------------------------------------------------

HEADLINE = "| | Recall | Precision | Line hit | Latency p50 / p95 | Judge calls | Cost |"


def pct(v) -> str:
    return "–" if v is None else f"{v * 100:.1f}%"


def secs(v) -> str:
    return "–" if v is None else f"{v:.1f}s"


def usd(v) -> str:
    """Dollars, with enough precision that a sub-cent Jev run doesn't read $0.00."""
    v = v or 0.0
    return f"${v:.2f}" if v == 0 or v >= 0.01 else f"${v:.6f}"


def headline_row(label: str, m: dict) -> str:
    """One headline row. Tokens stay in the JSON only: they are not comparable
    across backends (a harness counts its own cached system prompt)."""
    return (
        f"| {label} | {pct(m.get('recall'))} | {pct(m.get('precision'))} | "
        f"{pct(m.get('line_hit_rate'))} | {secs(m.get('latency_p50_s'))} / "
        f"{secs(m.get('latency_p95_s'))} | {m.get('judge_calls', 0)} | {usd(m.get('cost_usd'))} |"
    )


def delta_row(new: dict, old: dict) -> str:
    def dp(k):
        n, o = new.get(k), old.get(k)
        return "" if n is None or o is None else f"{(n - o) * 100:+.1f}pp"

    def ds(k):
        n, o = new.get(k), old.get(k)
        return "" if n is None or o is None else f"{n - o:+.1f}s"

    return (
        f"| Δ | {dp('recall')} | {dp('precision')} | {dp('line_hit_rate')} | "
        f"{ds('latency_p50_s')} / {ds('latency_p95_s')} | "
        f"{new.get('judge_calls', 0) - old.get('judge_calls', 0):+d} | "
        f"{(new.get('cost_usd') or 0.0) - (old.get('cost_usd') or 0.0):+.4f} |"
    )


def markdown(result: dict, baseline: dict | None) -> str:
    meta = result["meta"]
    m = result["overall"]
    lines = [
        f"### llmlint eval — arm `{meta['arm']}`",
        "",
        f"{meta['cases']} cases, {m['items']} scored items "
        f"({meta['repeats']} repeat(s)), model `{meta['model'] or 'harness default'}`, "
        f"llmlint `{meta['llmlint_version']}`. Errored items: {m.get('errors', 0)}; "
        f"unstable across repeats: {m.get('unstable_items', 0)}.",
        "",
        HEADLINE,
        "|---|---|---|---|---|---|---|",
    ]
    if baseline:
        b = baseline["overall"]
        bmeta = baseline.get("meta", {})
        lines.append(headline_row(f"baseline (`{bmeta.get('arm', '?')}`)", b))
        lines.append(headline_row(f"this run (`{meta['arm']}`)", m))
        lines.append(delta_row(m, b))
    else:
        lines.append(headline_row(f"`{meta['arm']}`", m))
    lines += ["", "| Suite | Recall | Precision | Line hit | Latency p50 |", "|---|---|---|---|---|"]
    for suite, sm in result["suites"].items():
        lines.append(
            f"| {suite} | {pct(sm['recall'])} | {pct(sm['precision'])} | "
            f"{pct(sm['line_hit_rate'])} | {secs(sm['latency_p50_s'])} |"
        )
    misses = [
        i
        for i in result["items"]
        if i["error"] or i["predicted"] != i["expected_violation"] or i.get("line_hit") is False
    ]
    if misses:
        lines += ["", "<details><summary>Misses</summary>", ""]
        for i in misses:
            if i["error"]:
                why = "errored"
            elif i["predicted"] != i["expected_violation"]:
                why = "missed violation" if i["expected_violation"] else "false alarm"
            else:
                why = "wrong line"
            lines.append(f"- `{i['case']}` · `{i['rule']}` (rep {i['rep']}): {why}")
        lines += ["", "</details>"]
    return "\n".join(lines) + "\n"


# --- CLI ---------------------------------------------------------------------


def resolve_bin(explicit: str | None, *candidates: Path, path_name: str | None = None) -> Path | None:
    """An explicit path, else the first existing candidate, else `path_name` on PATH."""
    if explicit:
        return Path(explicit)
    for c in candidates:
        if c.is_file():
            return c
    found = shutil.which(path_name) if path_name else None
    return Path(found) if found else None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--arm", choices=ARMS, default="harness")
    ap.add_argument("--suite", action="append", help="limit to a suite (repeatable)")
    ap.add_argument(
        "--split",
        choices=("all", "dev", "test"),
        default="all",
        help="dev = cases to tune on; test = held-out cases to report (default all)",
    )
    ap.add_argument("--repeats", type=int, default=1, help="runs per case (stability)")
    ap.add_argument("-j", "--jobs", type=int, default=4, help="cases run concurrently")
    ap.add_argument("--model", help="harness model override (harness arm)")
    ap.add_argument("--timeout", type=int, default=300, help="per-judge timeout (s)")
    ap.add_argument("--line-tolerance", type=int, default=1)
    ap.add_argument("--llmlint", help="llmlint binary (default: target/release, then PATH)")
    ap.add_argument("--oneharness", help="real oneharness (default: PATH)")
    ap.add_argument("--mock", help="mock-oneharness (default: target/release)")
    ap.add_argument("--baseline", help="a previous result JSON to diff against")
    ap.add_argument("-q", "--quiet", action="store_true", help="print only failures")
    ap.add_argument("--out", help="result JSON path (default: evals/results/<ts>-<arm>.json)")
    ap.add_argument(
        "--expect-perfect",
        action="store_true",
        help="exit 1 unless every metric is perfect (the oracle self-test)",
    )
    ap.add_argument(
        "--expect-blind",
        action="store_true",
        help="exit 1 unless recall is 0 with no errors (the blind self-test)",
    )
    a = ap.parse_args(argv)

    exe = ".exe" if os.name == "nt" else ""
    rel = REPO / "target" / "release"
    llmlint = resolve_bin(a.llmlint, rel / f"llmlint{exe}", path_name="llmlint")
    if not llmlint:
        print("eval: llmlint binary not found (build it: cargo build --release)", file=sys.stderr)
        return 2
    oneharness = mock = None
    if a.arm in ("jev", "cascade") and not os.environ.get("TYPESAFE_API_KEY", "").strip():
        print(f"eval: the {a.arm} arm needs TYPESAFE_API_KEY set", file=sys.stderr)
        return 2
    if a.arm in ("harness", "cascade"):
        oneharness = resolve_bin(a.oneharness, path_name="oneharness")
        if not oneharness:
            print("eval: oneharness not found (install it, or pass --oneharness)", file=sys.stderr)
            return 2
    elif a.arm == "jev":
        pass
    else:
        mock = resolve_bin(a.mock, rel / f"llmlint-mock-oneharness{exe}")
        if not mock:
            print(
                "eval: mock oneharness not found (cargo build --release --features "
                "mock-oneharness --bin llmlint-mock-oneharness)",
                file=sys.stderr,
            )
            return 2

    try:
        cases = load_corpus(CORPUS, a.suite)
    except (ValueError, KeyError, tomllib.TOMLDecodeError) as e:
        print(f"eval: bad corpus: {e}", file=sys.stderr)
        return 2
    if a.split != "all":
        cases = [c for c in cases if c.split == a.split]
    opts = Opts(llmlint, oneharness, mock, a.model, a.timeout)

    jobs = [(c, rep) for rep in range(a.repeats) for c in cases]
    runs: list[Run] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
        futs = {pool.submit(run_case, c, a.arm, rep, opts): (c, rep) for c, rep in jobs}
        for n, fut in enumerate(concurrent.futures.as_completed(futs), 1):
            run = fut.result()
            runs.append(run)
            status = {0: "pass", 1: "fail"}.get(run.exit, f"exit {run.exit}")
            if not a.quiet:
                print(f"[{n}/{len(jobs)}] {run.case} rep {run.rep}: {status} ({run.seconds:.1f}s)", file=sys.stderr)
            if run.exit not in (0, 1):
                print(f"{run.case} rep {run.rep}: {status}", file=sys.stderr)
                print("    " + run.stderr.strip().replace("\n", "\n    "), file=sys.stderr)
    runs.sort(key=lambda r: (r.case, r.rep))

    version = subprocess.run([str(llmlint), "--version"], capture_output=True, text=True).stdout.strip()
    result = summarize(cases, runs, a.line_tolerance)
    result["meta"] = {
        "arm": a.arm,
        "model": {
            "jev": os.environ.get("LLMLINT_JEV_MODEL") or "jev-1.13.0",
            "cascade": f"{os.environ.get('LLMLINT_JEV_MODEL') or 'jev-1.13.0'} + {a.model or 'harness default'}",
        }.get(a.arm, a.model),
        "repeats": a.repeats,
        "split": a.split,
        "cases": len(cases),
        "llmlint_version": version,
        "timestamp": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
    }
    result["runs"] = [
        {"case": r.case, "rep": r.rep, "exit": r.exit, "seconds": r.seconds, "usage": r.usage}
        for r in runs
    ]

    out = Path(a.out) if a.out else RESULTS / (
        dt.datetime.now().strftime("%Y%m%d-%H%M%S") + f"-{a.arm}.json"
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=2) + "\n")
    baseline = json.loads(Path(a.baseline).read_text()) if a.baseline else None
    md = markdown(result, baseline)
    out.with_suffix(".md").write_text(md)
    if not a.quiet:
        print(md)
        print(f"eval: wrote {out}", file=sys.stderr)

    m = result["overall"]
    if m["errors"]:
        print(f"eval: {m['errors']} item(s) could not be judged (see the errors above)", file=sys.stderr)
        return 1
    if a.expect_perfect:
        perfect = (
            m["errors"] == 0
            and m["recall"] == 1.0
            and m["precision"] == 1.0
            and m["line_hit_rate"] in (1.0, None)
            and m["unstable_items"] == 0
        )
        if not perfect:
            print("eval: oracle self-test FAILED: a perfect judge did not score perfectly", file=sys.stderr)
            return 1
    if a.expect_blind and not (m["errors"] == 0 and m["recall"] == 0.0 and m["fp"] == 0):
        print("eval: blind self-test FAILED: a never-flagging judge must score recall 0", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
