"""Unit tests for the eval runner's pure scoring (`python3 -m unittest evals/test_run.py`).

The end-to-end plumbing is covered by the oracle/blind self-tests (`just
eval-selftest`); these pin the scoring rules those can't distinguish — line
tolerance, file matching, error handling, and stability accounting.
"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import run  # noqa: E402
from run import Case, Expect, Run  # noqa: E402


def report(outcome, violations=(), name="r"):
    return {"rules": [{"name": name, "outcome": outcome, "violations": list(violations)}]}


def mk_run(case="s/c", outcome="fail", violations=(), exit_code=None, rep=0, seconds=1.0, usage=None):
    if exit_code is None:
        exit_code = 1 if outcome == "fail" else 0
    return Run(case, rep, exit_code, seconds, report(outcome, violations), "", usage or {})


VIOLATED = Expect("r", False, (10,), "src/a.rs")
HOLDS = Expect("r", True)


class LineHit(unittest.TestCase):
    def test_exact_and_within_tolerance(self):
        self.assertTrue(run.line_hit(VIOLATED, [{"file": "src/a.rs", "line": 10}], 0))
        self.assertTrue(run.line_hit(VIOLATED, [{"file": "src/a.rs", "line": 11}], 1))
        self.assertFalse(run.line_hit(VIOLATED, [{"file": "src/a.rs", "line": 12}], 1))

    def test_range_covers_planted_line(self):
        self.assertTrue(run.line_hit(VIOLATED, [{"file": "src/a.rs", "line": 5, "end_line": 20}], 0))

    def test_other_file_is_a_miss_but_normalized_paths_match(self):
        self.assertFalse(run.line_hit(VIOLATED, [{"file": "src/b.rs", "line": 10}], 1))
        self.assertTrue(run.line_hit(VIOLATED, [{"file": "./src/a.rs", "line": 10}], 0))
        self.assertTrue(run.line_hit(VIOLATED, [{"file": "src\\a.rs", "line": 10}], 0))

    def test_unlocated_violation_never_hits(self):
        self.assertFalse(run.line_hit(VIOLATED, [{"message": "somewhere"}], 5))


class JudgeItem(unittest.TestCase):
    def test_true_positive_scores_line(self):
        it = run.judge_item(VIOLATED, mk_run(violations=[{"file": "src/a.rs", "line": 10}]), 1)
        self.assertEqual((it["predicted"], it["error"], it["line_hit"]), (True, False, True))

    def test_pass_on_violation_is_a_miss_without_line_score(self):
        it = run.judge_item(VIOLATED, mk_run(outcome="pass"), 1)
        self.assertEqual((it["predicted"], it["error"]), (False, False))
        self.assertNotIn("line_hit", it)

    def test_not_relevant_counts_as_not_flagged(self):
        it = run.judge_item(HOLDS, mk_run(outcome="not_relevant"), 1)
        self.assertEqual(it["predicted"], False)

    def test_exit_2_missing_or_skipped_rule_is_an_error(self):
        self.assertTrue(run.judge_item(HOLDS, mk_run(outcome="pass", exit_code=2), 1)["error"])
        self.assertTrue(run.judge_item(Expect("other", True), mk_run(outcome="pass"), 1)["error"])
        self.assertTrue(run.judge_item(HOLDS, mk_run(outcome="skipped"), 1)["error"])
        broken = Run("s/c", 0, 1, 1.0, None, "boom")
        self.assertTrue(run.judge_item(HOLDS, broken, 1)["error"])


class Metrics(unittest.TestCase):
    def case(self, *expects, name="c"):
        return Case("s", name, Path("."), Path("x"), "file", "", tuple(expects))

    def test_confusion_counts_and_ratios(self):
        cases = [self.case(VIOLATED, name="v"), self.case(HOLDS, name="h")]
        runs = [
            mk_run("s/v", "fail", [{"file": "src/a.rs", "line": 30}]),  # TP, wrong line
            mk_run("s/h", "fail"),  # FP
        ]
        m = run.summarize(cases, runs, 1)["overall"]
        self.assertEqual((m["tp"], m["fp"], m["fn"], m["tn"]), (1, 1, 0, 0))
        self.assertEqual((m["recall"], m["precision"], m["line_hit_rate"]), (1.0, 0.5, 0.0))

    def test_errors_are_excluded_from_ratios_but_counted(self):
        cases = [self.case(VIOLATED)]
        m = run.summarize(cases, [mk_run(outcome="pass", exit_code=2)], 1)["overall"]
        self.assertEqual((m["errors"], m["recall"]), (1, None))

    def test_disagreeing_repeats_are_unstable(self):
        cases = [self.case(VIOLATED)]
        runs = [mk_run(outcome="fail", rep=0), mk_run(outcome="pass", rep=1)]
        self.assertEqual(run.summarize(cases, runs, 1)["overall"]["unstable_items"], 1)

    def test_latency_and_cost_totals(self):
        cases = [self.case(HOLDS)]
        runs = [
            mk_run(outcome="pass", seconds=2.0, rep=0, usage={"calls": 1, "cost_usd": 0.5, "input_tokens": 10}),
            mk_run(outcome="pass", seconds=4.0, rep=1, usage={"calls": 1, "cost_usd": 0.25, "output_tokens": 5}),
        ]
        m = run.summarize(cases, runs, 1)["overall"]
        self.assertEqual((m["latency_p50_s"], m["judge_calls"], m["cost_usd"], m["tokens"]), (3.0, 2, 0.75, 15))


class UsageOf(unittest.TestCase):
    def test_sums_every_backend_in_the_report(self):
        report = {"usage": {
            "harness": {"calls": 2, "input_tokens": 10, "cache_write_tokens": 500, "cost_usd": 0.3, "duration_ms": 9000},
            "jev": {"calls": 5, "input_tokens": 2000, "output_tokens": 20, "cost_usd": 0.0001, "duration_ms": 800},
        }}
        u = run.usage_of(report)
        self.assertEqual((u["calls"], u["input_tokens"], u["cache_write_tokens"], u["duration_ms"]), (7, 2010, 500, 9800))
        self.assertAlmostEqual(u["cost_usd"], 0.3001)

    def test_no_report_or_no_usage_is_zero(self):
        for r in (None, {}, {"usage": {}}):
            self.assertEqual(run.usage_of(r)["calls"], 0)


class Corpus(unittest.TestCase):
    def write_case(self, root, toml, files=True):
        suite = root / "s"
        case = suite / "cases" / "c"
        case.mkdir(parents=True)
        (suite / "llmlint.yml").write_text("version: 1\n")
        (case / "case.toml").write_text(toml)
        if files:
            (case / "files").mkdir()
        return root

    def test_rejects_malformed_cases(self):
        bad = [
            ('[[expect]]\nrule = "r"\nholds = true\nlines = [3]\n', "holds but lists"),
            ('[[expect]]\nrule = "r"\nholds = false\nlines = [3]\n', "without a file"),
            ('mode = "diff"\n[[expect]]\nrule = "r"\nholds = true\n', "needs base/"),
            ('description = "x"\n', r"no \[\[expect\]\]"),
            ('[[expect]]\nrule = "r"\nholds = false\nfile = "a.rs"\nlines = [1]\n', "names missing file"),
        ]
        for toml, msg in bad:
            with self.subTest(msg=msg), tempfile.TemporaryDirectory() as tmp:
                root = self.write_case(Path(tmp), toml)
                with self.assertRaisesRegex(ValueError, msg):
                    run.load_corpus(root)

    def test_the_committed_corpus_loads(self):
        cases = run.load_corpus(run.CORPUS)
        self.assertGreater(len(cases), 0)
        self.assertEqual(len({c.id for c in cases}), len(cases))


class Markdown(unittest.TestCase):
    def test_headline_with_baseline_delta(self):
        cur = {"overall": {"items": 1, "recall": 0.9, "precision": 1.0, "line_hit_rate": 1.0,
                           "latency_p50_s": 0.4, "latency_p95_s": 0.9, "judge_calls": 3, "cost_usd": 0.01},
               "suites": {}, "items": [],
               "meta": {"arm": "jev", "cases": 1, "repeats": 1, "model": None, "llmlint_version": "v"}}
        base = {"overall": {"recall": 0.95, "precision": 1.0, "line_hit_rate": 1.0,
                            "latency_p50_s": 8.1, "latency_p95_s": 12.6, "judge_calls": 3, "cost_usd": 4.73},
                "meta": {"arm": "harness"}}
        md = run.markdown(cur, base)
        self.assertIn("| baseline (`harness`) | 95.0% | 100.0% | 100.0% | 8.1s / 12.6s | 3 | $4.73 |", md)
        self.assertIn("| this run (`jev`) | 90.0% | 100.0% | 100.0% | 0.4s / 0.9s | 3 | $0.01 |", md)
        self.assertIn("| Δ | -5.0pp | +0.0pp | +0.0pp | -7.7s / -11.7s | +0 | -4.72 |", md)
        self.assertNotIn("Tokens", md)

if __name__ == "__main__":
    unittest.main()
