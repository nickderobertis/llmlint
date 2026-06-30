# llmlint

**The next generation of linting: an LLM as a judge.** `llmlint` enforces the
code-quality checks a human reviewer normally makes — adherence to architectural
patterns, coding-style intent, alignment to organization objectives — that
deterministic linters can't express. It is **additive** to your existing linters,
not a replacement: keep using deterministic tools for everything they can already
check, and reach for llmlint only for the judgment calls.

Each check is a **rule**: a statement about your code that is judged `true`
(holds) or `false` (a violation). llmlint batches your rules, drives a real coding
harness (Claude Code, Codex, Cursor, …) through
[`oneharness`](https://github.com/nickderobertis/oneharness) to read the relevant
files and decide, and reports the violations — with file and line numbers where
they can be pinned down. Because the gate is "just a config file," llmlint drops
into CI next to your other linters.

By default llmlint reports the failing rules (with the locations it could pin
down) and a one-line summary — passing and skipped rules are just counted:

![llmlint's default report: a red FAIL with its pinned violation and a colorized summary line](docs/screenshots/lint-default.svg)

Add `-v` to itemize *every* rule (passed and skipped too) and to print the
oneharness debug view — the exact `oneharness run …` command and the raw result
for each judge — to **stderr**, so the report on stdout stays clean:

![llmlint's verbose report: green PASS, red FAIL, and yellow SKIP lines above the summary](docs/screenshots/lint-verbose.svg)

<details>
<summary>The <code>-v</code> debug view (oneharness command + raw result per judge, on stderr)</summary>

![llmlint's -v oneharness debug view: the exact oneharness run command and the raw JSON result for each judge](docs/screenshots/lint-debug.svg)

</details>

> These are real captures of the CLI, rendered from the actual colorized output
> by [`just screenshots`](screenshots/AGENTS.md) and gated by
> [screencomp](https://github.com/nickderobertis/screencomp).

The exit code is unaffected by verbosity (`0` all-pass, `1` a violation, `2`
the run couldn't complete); operational errors are always shown. Use
`--format json` for the full machine-readable report.

The human report is **colorized** — green `PASS`, red `FAIL`/`ERROR` — when
stdout is a terminal. Coloring follows the [`NO_COLOR`](https://no-color.org)
convention and a `--color <auto|always|never>` flag: `auto` (the default) colors
only an interactive terminal, `always` forces it (e.g. through a pager or to
capture a screenshot), `never` disables it. `--format json` is never colorized.

## How it works

1. You declare **rules** (and optionally **agents** that group them) in a YAML
   config — like any other linter.
2. For each agent, llmlint renders a system prompt from a template (the rules +
   the target file paths) and calls `oneharness run` with a generated **JSON
   Schema** for structured output. oneharness constrains and validates the
   harness's answer, so llmlint gets a checked verdict per rule, not prose.
3. The harness reads the target files on demand with its own tools to gather
   evidence, then returns `{ "rule_name": { "holds": bool, "violations": [...] } }`.
4. llmlint aggregates (majority vote across judges when configured), reports, and
   exits non-zero if any rule was violated.

llmlint **shells out to oneharness** — it is a runtime prerequisite (see Install).

## Install

`llmlint` needs the `oneharness` binary on your `PATH`.

```console
# 1) oneharness (the harness driver)
curl -fsSL https://raw.githubusercontent.com/nickderobertis/oneharness/main/scripts/install.sh | sh
#    (or: cargo install --git https://github.com/nickderobertis/oneharness --locked)

# 2) llmlint
curl -fsSL https://raw.githubusercontent.com/nickderobertis/llmlint/main/scripts/install.sh | sh
#    (or: cargo install llmlint --locked)
#    (or, without a crates.io release: cargo install --git https://github.com/nickderobertis/llmlint --locked)

llmlint doctor      # confirms oneharness is reachable
```

The installer honors `LLMLINT_VERSION` / `LLMLINT_INSTALL_DIR` (or the `--version`
/ `--to` flags), works on Linux, macOS, and Windows under a POSIX shell
(Git Bash / MSYS / WSL), and refuses an archive whose checksum does not match.
Each tagged release publishes prebuilt, checksummed binaries for those
platforms; on native Windows PowerShell, use `cargo install llmlint --locked`.

You also need a coding harness installed and authenticated (e.g. Claude Code).
See `oneharness list` / `oneharness detect --all`.

## Quick start

```console
llmlint init                 # write a starter llmlint.yml (config-lint plugin on)
llmlint init --with-template # ...and embed the prompt template to customize
$EDITOR llmlint.yml          # write your rules
llmlint                      # lint the configured files
llmlint src/api/**/*.rs      # ...or lint specific files
llmlint --format json        # machine-readable output
```

## Configuration

`llmlint.yml` (discovered by walking up from the working directory; override with
`-c/--config`, repeatable). Discovery is **nested** in both directions. Walking
**up**, *every* config found (one per directory) is merged, nearest first, so a
config beside the files being linted, a project config above it, and a user-level
config higher still layer together — the most-local config wins each top-level
scalar, every config contributes its rules, and a more distant config fills only
the gaps (the same nearest-wins precedence as [plugins](#plugins-shared-rule-sets)).
Walking **down**, a config in a subdirectory governs *its own* part of the project:
its `files` globs are rooted at that directory (a `frontend/llmlint.yml` with
`*.txt` matches `frontend/`'s files, never a same-named file elsewhere), so you can
keep per-area rules next to the code they check. A subtree config scopes *rules*,
not session-wide settings (model, timeout, template, rationales come from the
working directory and up); `--config` replaces the whole walk with no cascade.
`llmlint init` writes it with a leading
`# yaml-language-server: $schema=…` modeline pointing at llmlint's
[published JSON Schema](assets/llmlint.schema.json), so editors with the YAML
language server (e.g. VS Code's [YAML extension](https://marketplace.visualstudio.com/items?itemName=redhat.vscode-yaml))
give completion and validation as you write. Add the same line to a hand-written
config to opt in.

```yaml
version: 1                     # this config's published version (used when it is consumed as a plugin)

# Files linted when none are passed on the CLI.
files:
  include: ["src/**/*.rs"]
  exclude: ["**/generated/**"]

# Require a short `rationale` for every verdict (default true). See Rationales below.
rationales: true

# Pull in shared rule sets / plugins with one line each. An entry is a local
# path or a URL (`http(s)://`, `file://`); pin a URL to a version with `@`.
plugins:
  - "https://raw.githubusercontent.com/nickderobertis/llmlint/main/assets/config_lint.yml@1"  # bundled: lints this config's own rules
  - "https://example.com/org-rules.yml@1.2.3"   # pinned; fetched + cached once
  - "./team-rules.yml"

# Agents group rules and add reviewer context + harness/model/batch config.
# YAML anchors let you share prompt text with zero framework support.
agents:
  architecture:
    harness: claude-code       # any id from `oneharness list`; omit to use oneharness's own default
    model: opus
    batch_size: 15             # rules per judge run (default 20)
    prompt_template: |         # appended to the master template before render
      You are a senior software architect reviewing service boundaries.

rules:
  - name: handlers_delegate_to_services   # unique, terse, descriptive
    description: |
      true when every HTTP handler delegates business logic to a service layer.
      false when a handler performs business logic (DB queries, domain rules)
      inline.
    agent: architecture        # optional; omit to use the default agent
    # override: true           # optional; extend a same-named plugin rule, inheriting unset fields
    judges: 3                  # optional; independent judges, majority wins (default 1)
    rationale: true            # optional; override the session-wide `rationales` for this rule
    relevance: true            # optional; when to evaluate — see Relevance below (default true)
    files:                     # optional; override the target files for this rule
      include: ["src/api/**"]
```

### Nested & per-directory configs

Configs **nest** — discovery walks both up from the working directory and down
into its subtree, merging every `llmlint.yml` it finds. This lets you layer a
user-level config, a project config, and per-area configs that live next to the
code they govern, with no extra wiring.

```
~/.llmlint.yml                 # user-level defaults (model, rationales…)
my-project/
├── llmlint.yml                # project rules + settings  (run from here)
├── backend/
│   └── llmlint.yml            # rules for backend/**, globs rooted at backend/
│       #   files: { include: ["**/*.py"] }
│       #   rules: [{ name: no_print_debugging, … }]
└── frontend/
    └── llmlint.yml            # rules for frontend/**, globs rooted at frontend/
        #   files: { include: ["**/*.ts"] }
        #   rules: [{ name: no_inline_styles, … }]
```

Running `llmlint` from `my-project/` evaluates **all** of these together:

- `no_print_debugging` runs only on `backend/**/*.py`, and `no_inline_styles`
  only on `frontend/**/*.ts` — each subtree config's `files` globs are **rooted
  at its own directory**, so `**/*.py` under `backend/llmlint.yml` means
  `backend/**/*.py`, never a stray `.py` elsewhere.
- The project's own rules and settings apply across the whole run; a more-local
  config **wins** each setting and can `override` a rule from a config above it.
- **Session settings** (model, timeout, prompt template, rationales) come from
  the working directory and up — a subtree config scopes *rules*, it doesn't
  retune the whole run. Run from `my-project/backend/` instead and that config
  becomes the most-local one, layering under the project and user configs.

Use `llmlint config` to see the merged result and `llmlint config --sources`
(or `llmlint where rules.<name>`) to trace any rule, agent, or setting back to
the exact file it came from. To bypass discovery entirely, pass explicit
configs with `-c/--config` (repeatable) — that roots every glob at the working
directory with no cascade.

### Writing good rules

- **Phrase each rule as a positive invariant.** `holds = true` means the code
  complies; `holds = false` is a violation that llmlint reports and fails on.
- **Make the true/false outcome unambiguous and mutually exclusive** — state when
  it is true *and* when it is false. The bundled config-lint plugin (the
  `config_lint.yml` URL above) lints your config for exactly this, plus
  descriptive (non-placeholder) names that match what each rule checks.
- **Names** are unique, terse, and descriptive (`^[A-Za-z][A-Za-z0-9_]*$`); they
  become the JSON keys of the structured output.
- **Scope a rule to the changes it applies to with `relevance`** (see below)
  instead of bolting "…or not applicable" onto the description — that keeps the
  true/false outcome clean and lets llmlint tell "didn't apply" apart from "true".

### The prompt template

llmlint renders the judge's system prompt from a
[minijinja](https://docs.rs/minijinja) (Jinja2-style) template. The bundled
default lives in [`assets/default_template.md`](assets/default_template.md); embed
a copy to customize with `llmlint init --with-template`, or set `prompt_template`
yourself. The top-level `prompt_template` *replaces* the master template; an
agent's `prompt_template` is **appended** to it before rendering, so reviewer
context you add per-agent sees the same variables.

Three variables are in scope when a template renders:

| Variable | Type | Description |
| --- | --- | --- |
| `files` | list of strings | The target file paths for this run — relative to the working directory, always forward-slashed (so a Windows run reads the same as Linux/macOS). |
| `rules` | list of objects | The rules in this batch. Each has `.name` (the identifier, also the JSON key in the structured output), `.description` (the invariant to judge), `.rationale` (whether this rule wants a justification), and `.relevance` (the relevance condition string, or unset for an always-evaluated rule). |
| `rationales` | bool | True when any rule in this batch wants a rationale — gate the rationale guidance on it. |
| `relevance` | bool | True when any rule in this batch carries a relevance condition — gate the relevance guidance on it. |

```jinja
## Target files
{% for f in files %}- {{ f }}
{% endfor %}
## Rules to evaluate
{% for r in rules %}### {{ r.name }}

{{ r.description }}
{% endfor %}
```

A run is one `(agent, file set, judge)` batch, so `rules` is that batch's slice
(see `batch_size`), not necessarily every rule in the config.

### Rationales

By default each judge must justify every verdict with a short **rationale**. The
structured output for each rule is ordered deliberately — the judge echoes the
rule **name**, writes the **rationale**, then commits to the **result**
(`holds` + any `violations`):

```jsonc
{
  "no_inline_sql": {
    "name": "no_inline_sql",                       // 1. anchor on the rule
    "rationale": "raw SQL built inline in db.rs:42, not via the query layer",  // 2. reason
    "holds": false,                                // 3. conclude
    "violations": [{ "file": "src/db.rs", "line": 42, "message": "inline SQL" }]
  }
}
```

Reasoning *before* concluding (and naming the rule first) keeps each verdict
consistent and targeted — it leans on the model's next-token prediction so the
`holds` follows from the evidence just written, not the other way round. Beyond
that, rationales buy you:

- **Auditability** — a durable record of *why* each verdict landed, carried in
  `--format json` for every rule (pass or fail).
- **Debugging** — when a verdict looks wrong, you see the judge's reasoning, not
  just a bare pass/fail.
- **Reliability** — verdicts are measurably steadier when the judge must commit
  to evidence first.

The cost is **extra output tokens on every request**. Turn rationales off to
save tokens:

```yaml
rationales: false            # session-wide default (CLI --no-rationales overrides it)

rules:
  - name: handlers_delegate_to_services
    description: ...
    rationale: true           # …but keep them for this high-stakes rule
```

Precedence, lowest to highest: the session default `rationales` (default `true`)
→ a per-rule `rationale` → the `--rationales` / `--no-rationales` CLI flags
(which set the session default for the run; a per-rule `rationale` still wins).
In the human report, a rule's rationale is shown for every **failure** by
default, and for **every evaluated rule** at `-v`. The default prompt template
asks for rationales that are terse and pithy — the fewest tokens that still cite
the evidence — so the token cost stays small.

For a **multi-judge** rule (`judges: N`), the report and `--format json` show
**each judge's** result and rationale, not just one representative — so you can
see exactly where the judges agreed or split:

```text
FAIL no_inline_sql (1/3 judges held)
     judge 1 violated: raw SQL concatenated at db.rs:3
     judge 2 held: all access goes through the query builder
     judge 3 violated: f-string SQL in the helper
     src/db.rs:3: inline SQL
```

### Relevance

Not every rule applies to every change. Rather than make each `description`
carry its own "…or not applicable" escape hatch — which muddies the true/false
outcome and hides *why* a rule passed — declare when a rule should be evaluated
with **`relevance`**:

```yaml
rules:
  # Always evaluated (the default). The judge may not opt out.
  - name: public_items_are_documented
    description: ...
    # relevance: true            # implicit

  # Never evaluated — disabled deterministically, with no judge call.
  - name: legacy_only_check
    description: ...
    relevance: false

  # Conditionally evaluated. The judge decides whether the condition holds for
  # the change *before* the verdict; if it doesn't, the rule is "not relevant".
  - name: errors_are_contextualized
    description: |
      TRUE when every returned error adds context about the operation that
      failed. FALSE when an error is propagated with no added context.
    relevance: the change adds or modifies error handling
```

For a conditional rule the structured output gains a `relevant` boolean, decided
before the verdict — so a not-applicable rule is distinguishable from a true one:

```jsonc
// Not relevant: the object ends after `relevant`; the rationale explains why.
{ "errors_are_contextualized": {
    "name": "errors_are_contextualized",
    "rationale": "the change only renames a struct field; no error handling touched",
    "relevant": false } }

// Relevant: proceed to the verdict as usual.
{ "errors_are_contextualized": {
    "name": "errors_are_contextualized",
    "rationale": "every `?` propagation wraps with `.context(...)`",
    "relevant": true,
    "holds": true } }
```

A **not-relevant** rule is neither a pass nor a violation — it never fails the
build. The human report counts it in a `… not relevant` summary segment and, at
`-v`, itemizes it as a dim `N/A <rule> (not relevant)` line with the reason;
`--format json` carries `"outcome": "not_relevant"` and a `not_relevant` summary
count. For a multi-judge rule, relevance is decided by majority first, then the
verdict is tallied over the judges that found it relevant.

### Ignore directives

Suppress a rule at a specific place with an inline comment in the target file —
the same idea as `# noqa` / `// eslint-disable`, but **strict**: a directive must
name the specific rule(s) and give a reason.

```rust
let q = format!("SELECT * FROM users WHERE id = {id}"); // llmlint: ignore[no_inline_sql] one-off migration, not user-facing
```

```python
# llmlint: ignore-file[public_items_are_documented] generated stubs, documented upstream
```

```rust
// llmlint: ignore-block[no_inline_sql] legacy query layer, migration tracked in JIRA-42
fn legacy_queries() { /* … */ }
// llmlint: ignore-end[no_inline_sql]
```

- `llmlint: ignore[rule, ...] <reason>` is **line-scoped** — it covers the line it
  sits on (a trailing comment) or the line right below it (a comment on its own line).
- `llmlint: ignore-file[rule, ...] <reason>` is **file-scoped** — it covers the
  whole file.
- `llmlint: ignore-block[rule, ...] <reason>` … `llmlint: ignore-end[rule, ...]` is
  **block-scoped** — it covers every line between the open and its matching close.
  The closing `ignore-end` names the same rule(s) and needs no reason. Blocks track
  each rule independently, so rules opened together may be closed at different points
  and blocks for different rules may overlap.

Use whatever comment syntax the file's language uses (`//`, `#`, `/* … */`, `<!-- … -->`);
llmlint keys off the reserved `llmlint: ignore` / `llmlint: ignore-file` /
`llmlint: ignore-block` / `llmlint: ignore-end` prefix.

**Two layers, by design.** llmlint deterministically validates each directive's
*structure* before any judge runs — it must name **specific, configured** rule(s)
and carry a **reason** (except `ignore-end`, which only closes a block). A directive
with no brackets, an empty list, an unknown or misspelled rule, or no reason is a
hard `file:line:` error (exit 2), so a typo fails loudly instead of silently
suppressing nothing. Block pairing is checked too: an unclosed `ignore-block`, an
`ignore-end` with no open block, or re-opening a rule already open is a hard error. Actually *honoring* a
well-formed directive is the judge's job: the default prompt tells it to skip a
named rule's violation at the directive's location. (A custom `prompt_template`
should carry the same guidance if you want directives honored.) Because the
prefix is reserved, a *linted* file that merely documents the feature must use
real rule names or avoid the literal `llmlint: ignore[…]` form.

This structural check is **deterministic and free** — no model call — so it is
also exposed as its own command, [`llmlint check-ignores`](#commands--exit-codes).
Run it in your tight, fast linter loop (next to `cargo fmt` / `clippy`, in a
pre-commit hook, or as a quick CI step), where it catches a typo'd or
reason-less directive in milliseconds. The full `llmlint` run performs the same
check as a pre-flight, so the two never disagree — `check-ignores` just gives you
the fast feedback without waiting on (or paying for) a judge.

### Judges and voting

`judges: N` runs a rule through `N` independent judges and takes the **majority**
verdict. `N` must be **odd** (1, 3, 5, …) so the vote can't tie — an even count is
a config error. Only rules that opt in pay the extra cost: judge 1 runs all rules,
judge 2 only the rules with `judges >= 2`, and so on.

### oneharness passthrough

llmlint lets oneharness discover its own `oneharness.toml` by default. To force a
specific oneharness config, use `--oneharness-config <path>` (or `oneharness.config`
in the llmlint config); it is forwarded via oneharness's `--config`. Override the
binary with `--oneharness-bin` or `$LLMLINT_ONEHARNESS_BIN`.

### Plugins (shared rule sets)

`plugins` pulls other llmlint configs into this one — their rules and agents are
merged in. For the **top-level settings** (template, files, oneharness,
rationales), **the nearer config to the root wins**: your config's settings take
precedence over a plugin's, a plugin's over its own plugins', and an
earlier-listed plugin over a later sibling. A plugin only *fills in* a setting
the including config left unset, so a shared plugin can ship sensible defaults
without overriding what you set locally. The CLI overrides all of them (see
Commands). Each entry is a config file:

- a **local path** (`./team-rules.yml`), resolved relative to the including file;
- a **URL** — `http(s)://` (fetched over HTTPS) or `file://` (read directly).

Resolution is **transitive**: a pulled-in config's own `plugins` are pulled in
turn, and so on. Diamonds and cycles are de-duplicated (each config loads once),
and the chain is bounded at a depth of 100 to fail fast on a pathological graph.

By default a rule name is **unique** across the whole merged config — declaring
the same name twice is an error. To **adjust** a rule a plugin gave you without
restating it, re-declare it with `override: true` and set only the fields you
want to change; every other field (including the `description`) is inherited from
the plugin's rule:

```yaml
plugins:
  - "https://example.com/org-rules.yml@1"   # ships `no_inline_sql`, 1 judge

rules:
  # Keep the org rule's text, but vote it across 3 judges and scope it tighter.
  - name: no_inline_sql
    override: true
    judges: 3
    files:
      include: ["src/db/**"]
```

The override must be set on the **nearer-root** config, and there must be exactly
one base rule (the same name declared *without* `override`) for it to extend — an
`override` with nothing to override is an error, so a typo'd name can't silently
do nothing. When several configs override the same base, the nearest-root
override wins each field.

URL fetching is built in (a pure-Rust HTTPS client — no `curl` or other external
tools, no system OpenSSL) and honors the standard `HTTP(S)_PROXY` / `NO_PROXY`
env vars. The bundled config-lint plugin ships inside the binary and resolves
**offline**.

A URL may be **pinned to a version** with an `@` suffix matching the plugin
config's own top-level `version`: `@1` accepts any `1.x`, `@1.2` any `1.2.x`,
`@1.2.3` exactly that. The pin is both an assertion (a mismatch is a hard error)
and the **cache key**: a pinned URL is fetched once into the cache and reused on
later runs without refetching — bump the pin to pull a new version. An *unpinned*
URL is fetched every run.

The cache lives under `$XDG_CACHE_HOME/llmlint/plugins` (override with
`LLMLINT_CACHE_DIR`). Set `LLMLINT_PLUGIN_REFRESH=1` to force a refetch.

### Finding where something is defined

Once configs merge across files and plugins, a rule, agent, or setting in the
effective config can come from any of them. Two commands trace an item back to
the file (or plugin URL) you'd edit to change it.

`llmlint where <path>` answers one lookup and prints **just the source**, so it
composes in scripts. The path mirrors the config structure:

```console
$ llmlint where oneharness.model            # a top-level setting
./shared/team.yml
$ llmlint where agents.security             # an agent
./shared/team.yml
$ llmlint where rules.no_inline_sql         # where a rule is defined
https://example.com/org-rules.yml@1
$ llmlint where rules.no_inline_sql.judges  # the file an override set a field in
./llmlint.yml
$ editor "$(llmlint where rules.no_inline_sql.judges)"
```

Because an `override` resolves **field by field**, a single rule can draw its
`description` from the plugin that defined it and its `judges` from your config —
`where rules.<name>.<field>` points at the file that actually set that field (or
the definition site when no override did). An unknown name lists what's available,
and a setting left at its built-in default says so, both exiting non-zero.

For the whole picture at once, `llmlint config --sources` adds a `sources` block:

```jsonc
{
  "config_files": ["./llmlint.yml", "./shared/team.yml", "https://example.com/org-rules.yml@1"],
  "sources": {
    "settings": { "version": "./llmlint.yml", "oneharness.model": "./shared/team.yml" },
    "agents":   { "security": "./shared/team.yml" },
    "rules": {
      "no_inline_sql": {
        "source": "https://example.com/org-rules.yml@1",  // where the rule is defined
        "fields": { "judges": "./llmlint.yml" }            // a field an override moved
      }
    }
  },
  "config": { /* … the merged config … */ }
}
```

A rule with no cross-file override has no `fields` entry; settings and agents are
each kept whole from the nearest-root config that set them, so they have a single
source.

## Commands & exit codes

- `llmlint [FILES...]` — lint (the default). `--format human|json`, `--agent`,
  `--rule`, `--max-parallel`, `--timeout`, `--cwd`. Target individual rules with
  `--rule NAME` (repeatable) or a whole group with `--agent NAME`; an unknown
  rule/agent name is an exit-2 error that lists the available names. Every
  top-level setting also has a flag that wins over the config:
  `--rationales`/`--no-rationales`, `--model NAME`, `--schema-max-retries N`,
  `--prompt-template PATH`, plus `--oneharness-bin`/`--oneharness-config`.
- `llmlint check-ignores [FILES...]` — validate the *structure* of inline
  `llmlint: ignore` directives in the target files, **deterministically and with
  no model call** (`-c/--config`, `--cwd`; pass `FILES` to scope it, e.g. the
  changed files in a pre-commit hook). This is the same pre-flight `lint` runs,
  split out for the fast static-check loop: exit `0` when every directive is
  well-formed, exit `2` (located `file:line:`) on a typo'd / reason-less /
  unbalanced one.
- `llmlint init` — write a starter config (`--with-template`, `--global`, `--force`).

  ![llmlint init writing a starter llmlint.yml](docs/screenshots/init.svg)
- `llmlint config` — print the merged config and the ordered list of sources that
  contributed, as JSON. Add `--sources` to also trace every rule, agent, and
  setting back to the file (or plugin URL) it came from — see
  [Finding where something is defined](#finding-where-something-is-defined).

  ![llmlint config printing the merged config and its sources as JSON](docs/screenshots/config.svg)
- `llmlint where <path>` — print the single source of one config item: a setting
  (`oneharness.model`, `version`), `agents.<name>`, `rules.<name>`, or a rule
  field `rules.<name>.<field>`. See
  [Finding where something is defined](#finding-where-something-is-defined).
- `llmlint doctor` — check that oneharness is installed and reachable.

  ![llmlint doctor reporting the resolved oneharness version](docs/screenshots/doctor.svg)

Exit codes: `0` all rules hold · `1` at least one violation · `2` usage,
configuration, or harness error (could not complete the lint).

## Development

```console
just bootstrap   # toolchain components + fetch (from a clean clone)
just check       # full gate: fmt, clippy -D warnings, tests + 95% coverage, docs
just test-e2e    # the e2e binary journeys in isolation
just deps-check  # cargo deny + cargo machete
just lint-live   # opt-in: ad-hoc lint against the REAL oneharness + a real harness
just live-claude # opt-in: live e2e — built llmlint → real oneharness → real harness
```

Tests drive the real `llmlint` binary against a hermetic mock-oneharness fixture.
The live tier (`just live-claude`, and the ad-hoc `just lint-live`) drives the
whole stack end to end against a real, authenticated harness — the only thing that
makes real model calls, and out of the `check` gate. It runs on PRs in its own
workflow across Linux/macOS/Windows, so a missing CLI, auth, or oneharness is a
hard failure, not a skip. See `AGENTS.md` and `tests/AGENTS.md`.

## License

MIT — see [LICENSE](LICENSE).
