# Agent evals

These evals check that an AI agent with no prior knowledge can use
`ldraw2print` correctly and honestly, learning it only from the tool's own
`--help` and `guide` output. Each eval gives a fresh Claude Code session a
realistic user request. Automatic checks then score what it did.

Run them after changing the CLI, help text, guides, JSON output or anything
an agent relies on. They have already caught real bugs, for example
non-deterministic repairs and `--smooth` silently opening seams.

## Running

```bash
cargo build --release                        # evals use target/release/ldraw2print
python3 evals/run.py --list                  # tasks
python3 evals/run.py                         # all tasks, 3 at a time
python3 evals/run.py 04 06                   # some tasks (name or prefix)
python3 evals/run.py --model sonnet          # a specific model
python3 evals/run.py --check evals/runs/20261027-153000   # re-score an old run
```

Requirements:
- `claude` (Claude Code CLI) on PATH and logged in. Each task is one
  `claude -p` session, typically 1–3 minutes and 50–100k tokens.
- An LDraw library for tasks with `library: yes`. The runner uses
  `$EVAL_LDRAWDIR`, then `$LDRAWDIR`, then `~/ldraw`.
- Python 3.9 or newer (standard library only).

## Isolation

Each task runs in `evals/runs/<timestamp>/<task>/`:

| Path | Purpose |
|---|---|
| `work/` | the agent's working directory; its outputs land here |
| `home/` | the `HOME` that `ldraw2print` sees, with the task's config fixture |
| `bin/ldraw2print` | a wrapper on the agent's PATH that sets that HOME and the library |
| `transcript.jsonl` | the full `stream-json` transcript |
| `report.md` | the agent's final reply plus its usability review |
| `checks.json` | check results for this task |

The tool never sees your real `~/.config/ldraw2print/config.toml`. The runner
hashes that file before and after, and every task fails if it changed.
Sessions run with `--restricted`: Bash, Read, Write, Edit, Glob and Grep
only, file tools confined to the working directory, your Claude settings,
hooks and MCP servers ignored, and no web tools.

## Tasks

Each task is a document in `tasks/`:
- front matter with the setup:
  - `library: yes|no`
  - `config: none|default|calibrated` (a file in `fixtures/`)
  - `files:` fixtures copied into work/
- `## Prompt`: the user's request, verbatim
- `## Good outcome`: what a strong answer does
- `## Automatic checks`: what `checks.py` verifies

| Task | Tests |
|---|---|
| 01-pirate-hat | Discovery by theme, head-body connector, `--close-open-shells`, painted 3MF |
| 02-construction-helmet | Calibration status reporting (a calibrated config) |
| 03-torch | Exit code 4: choosing between connector candidates; `--smooth` seams |
| 04-first-run | No LDraw library: guides the user, doesn't fabricate or download unasked |
| 05-prusa-head | Non-Bambu slicer; painted 3MF only verified in Bambu Studio |
| 06-calibration-loop | Records a calibration in the config correctly, then uses it |
| 07-out-of-scope | Honest about limits (no articulated prints) |
| 08-manual-connector | Socket not built from primitives: `--connector-axis/--connector-radius` |

## Reading results

The runner prints PASS/FAIL per task, with each failed and warned check.
- A task **fails** if any check fails.
- **Warnings** are softer expectations, such as reading the source or
  skipping the usability review.

Always read `report.md` as well. Each agent ends with a usability review,
which is the best source of ideas for improving help and output. Some
judgements, such as task 07's alternative, need a human.

Results depend on the model, so compare runs made with the same `--model`.
Agents are non-deterministic, so a single failure is a signal to read the
transcript, not proof of a regression. Rerun the task before concluding.

## Results

See [RESULTS.md](RESULTS.md) for past runs, findings and deferred feature requests.
Add an entry after each run that changes the picture.

## Adding a task

1. Write `tasks/NN-name.md` following the existing ones. Put any fixture
   files in `fixtures/`.
2. Add a check function to `CHECKS` in `checks.py`. Check behaviour (commands
   run, files written, what the reply says, sandbox state), not exact wording.
3. Run it a few times; tighten the prompt or checks until passes and failures
   mean something.
