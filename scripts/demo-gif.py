#!/usr/bin/env python3
"""Render the animated demo GIF of llmlint's live-progress view (the README hero).

Like `scripts/screenshots.sh`, this drives the **real release `llmlint` binary**
against the mock-oneharness fixture (`screenshots/fixture/`) — so the rules,
verdicts, and final report are genuine CLI output, only the judge answers are
scripted (no model, no network, no cost). The live view animates too fast and
non-deterministically to screen-record hermetically, so instead of capturing a
PTY (which would need ttyd/ffmpeg) we reconstruct the exact frames the view draws
— the same glyphs, words, and status colors as `src/commands/progress.rs` — from a
real run's structured output, and render them with the **vendored, pinned
JetBrains Mono font** (`screenshots/fonts/`, the same one the SVG screenshots
use). The result is deterministic and self-contained (Pillow only).

The GIF is informational, like the screenshots — it is NOT hash-gated (a GIF is
not byte-reproducible across Pillow versions), so it is regenerated on demand with
`just screenshots-gif` and committed to `docs/screenshots/demo.gif`.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

# GitHub-dark palette, matching the SVG screenshots' window (bg #0d1117).
BG = (13, 17, 23)
BAR = (22, 27, 34)
FG = (201, 209, 217)
DIM = (139, 148, 158)
GREEN = (63, 185, 80)
RED = (248, 81, 73)
YELLOW = (210, 153, 34)
CYAN = (57, 197, 207)
DOTS = [(255, 95, 86), (255, 189, 46), (39, 201, 63)]  # traffic-light window dots

# A rotating quadrant-block spinner. The vendored JetBrains Mono renders these as
# four *distinct* glyphs (unlike braille or quadrant circles, which collapse to a
# single fallback glyph at this size — making the animation freeze).
SPINNER = "▖▘▝▗"
COLS = 78
FONT_SIZE = 20
PAD = 24
BAR_H = 40
FRAME_MS = 130      # per animation frame
HOLD_MS = 2600      # hold on the final report


def _run(binp: str, mock: str, fixture: str, extra: list[str]) -> subprocess.CompletedProcess:
    """Drive the real binary against the fixture with a *fresh* mock-state dir, so
    the fixture's per-judge verdict sequences replay from the start every run."""
    env = dict(os.environ)
    env["LLMLINT_MOCK_VERDICTS"] = str(Path(fixture) / "verdicts.json")
    env["LLMLINT_MOCK_STATE"] = tempfile.mkdtemp(prefix="llmlint-gif-")
    return subprocess.run(
        [binp, "-c", str(Path(fixture) / "llmlint.yml"), "--oneharness-bin", mock,
         "--max-parallel", "1", *extra],
        cwd=fixture, env=env, capture_output=True, text=True,
    )


def run_json(binp: str, mock: str, fixture: str) -> list[dict]:
    """Run the real binary against the fixture and return the report's rules."""
    return json.loads(_run(binp, mock, fixture, ["--format", "json"]).stdout)["rules"]


def run_report(binp: str, mock: str, fixture: str) -> list[str]:
    """The genuine plain-text final report (what the view clears to reveal)."""
    return _run(binp, mock, fixture, ["--color", "never"]).stdout.splitlines()


# A frame is a list of lines; a line is a list of (text, color) segments.
def status_seg(name: str, outcome: str) -> tuple[str, tuple[int, int, int]]:
    glyph, word, color = {
        "pass": ("✓", "passed", GREEN),
        "fail": ("✗", "failed", RED),
        "skipped": ("–", "skipped", YELLOW),
        "not_relevant": ("–", "not relevant", YELLOW),
    }[outcome]
    return (f"{glyph} {name}  {word}", color)


def build_frames(rules: list[dict], report: list[str]) -> list[tuple[list, int]]:
    judged = [r for r in rules if r["outcome"] in ("pass", "fail")]
    resolved_upfront = [r for r in rules if r["outcome"] in ("skipped", "not_relevant")]
    order = sorted(rules, key=lambda r: r["name"])
    total = sum(int(r.get("votes_total", 1)) for r in judged) or 1

    frames: list[tuple[list, int]] = []
    done_names: dict[str, str] = {r["name"]: r["outcome"] for r in resolved_upfront}
    running: str | None = None
    calls = 0
    spin = 0

    def render(spin_i: int) -> list:
        sp = SPINNER[spin_i % len(SPINNER)]
        lines = [[(f"{sp} judging {calls}/{total} judge calls", CYAN)]]
        lines.append([("", FG)])
        for r in order:
            name = r["name"]
            if name in done_names:
                lines.append([status_seg(name, done_names[name])])
            elif name == running:
                lines.append([(f"{sp} ", CYAN), (f"{name}  running", FG)])
            else:
                lines.append([(f"{sp} ", CYAN), (f"{name}  ", FG), ("queued", DIM)])
        return lines

    # Opening beat: everything queued.
    for _ in range(3):
        frames.append((render(spin), FRAME_MS))
        spin += 1

    for r in judged:
        running = r["name"]
        for _ in range(3):
            frames.append((render(spin), FRAME_MS))
            spin += 1
        running = None
        calls += int(r.get("votes_total", 1))
        done_names[r["name"]] = r["outcome"]
        frames.append((render(spin), FRAME_MS))
        spin += 1

    # Settle on the fully-resolved view briefly...
    for _ in range(3):
        frames.append((render(spin), FRAME_MS))
        spin += 1

    # ...then the view clears and the genuine report is revealed.
    report_lines: list = []
    for line in report:
        report_lines.append(colorize_report(line))
    frames.append((report_lines, HOLD_MS))
    return frames


def colorize_report(line: str) -> list:
    """Colorize a plain report line the way the human report would."""
    if line.startswith("FAIL") or line.startswith("ERROR"):
        head, _, rest = line.partition(" ")
        return [(head, RED), (" " + rest, FG)]
    if line.startswith("PASS"):
        head, _, rest = line.partition(" ")
        return [(head, GREEN), (" " + rest, FG)]
    if line.startswith("N/A") or line.startswith("SKIP"):
        head, _, rest = line.partition(" ")
        return [(head, YELLOW), (" " + rest, FG)]
    if "rules:" in line:
        return [(line, FG)]
    if line.strip().startswith("judge") or ":" in line and line.startswith("     "):
        return [(line, DIM)]
    return [(line, FG)]


def render_gif(frames: list[tuple[list, int]], font_path: str, out: str) -> None:
    font = ImageFont.truetype(font_path, FONT_SIZE)
    cw = int(font.getlength("M"))
    asc, desc = font.getmetrics()
    lh = asc + desc + 6
    rows = max(len(f[0]) for f in frames)
    width = PAD * 2 + COLS * cw
    height = BAR_H + PAD + rows * lh + PAD

    def draw_frame(lines: list) -> Image.Image:
        img = Image.new("RGB", (width, height), BG)
        d = ImageDraw.Draw(img)
        # Window chrome: a title bar with three traffic-light dots.
        d.rectangle([0, 0, width, BAR_H], fill=BAR)
        for i, col in enumerate(DOTS):
            cx = PAD + i * 22
            cy = BAR_H // 2
            d.ellipse([cx - 6, cy - 6, cx + 6, cy + 6], fill=col)
        y = BAR_H + PAD
        for segs in lines:
            x = PAD
            for text, color in segs:
                d.text((x, y), text, font=font, fill=color)
                x += int(font.getlength(text))
            y += lh
        return img

    imgs = [draw_frame(lines) for lines, _ in frames]
    durations = [ms for _, ms in frames]
    imgs[0].save(
        out, save_all=True, append_images=imgs[1:], duration=durations,
        loop=0, optimize=True, disposal=2,
    )


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    binp = os.environ.get("LLMLINT_BIN", str(root / "target/release/llmlint"))
    mock = os.environ.get("LLMLINT_MOCK_BIN", str(root / "target/release/llmlint-mock-oneharness"))
    fixture = str(root / "screenshots/fixture")
    font_path = str(root / "screenshots/fonts/JetBrainsMono-Regular.ttf")
    out = os.environ.get("DEMO_GIF_OUT", str(root / "docs/screenshots/demo.gif"))

    for p in (binp, mock, font_path):
        if not Path(p).exists():
            print(f"demo-gif: missing {p}", file=sys.stderr)
            return 1

    rules = run_json(binp, mock, fixture)
    report = run_report(binp, mock, fixture)
    frames = build_frames(rules, report)
    Path(out).parent.mkdir(parents=True, exist_ok=True)
    render_gif(frames, font_path, out)
    print(f"demo-gif: wrote {out} ({len(frames)} frames)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
