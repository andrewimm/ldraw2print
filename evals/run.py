#!/usr/bin/env python3
"""Run ldraw2print agent evals: fresh `claude -p` sessions use the tool from
its own help to do realistic user requests, then automatic checks score
the transcripts and outputs.

    python3 evals/run.py                     # all tasks, 3 in parallel
    python3 evals/run.py 04-first-run 06     # tasks by name or prefix
    python3 evals/run.py --check evals/runs/20261027-1530   # re-check a run
    python3 evals/run.py --list

Each task runs in evals/runs/<timestamp>/<task>/:
    work/        the agent's working directory (outputs land here)
    home/        HOME seen by ldraw2print (task config; no access to yours)
    bin/         an `ldraw2print` wrapper on the agent's PATH
    transcript.jsonl, report.md, checks.json
See evals/README.md.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import datetime as dt
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

EVALS = Path(__file__).resolve().parent
REPO = EVALS.parent
TOOL = REPO / "target" / "release" / "ldraw2print"
TASKS = EVALS / "tasks"
RUNS = EVALS / "runs"

PREAMBLE = """You are an AI assistant helping a user with a 3D-printing request. A command-line \
tool called `ldraw2print` is installed on your PATH. Learn how to use it ONLY from its own \
help and guide output (`ldraw2print --help`, `ldraw2print guide …`). Do not read its source \
code or README. Work in the current directory; put any files you create here. Do not run \
`ldraw2print preview` (it is an interactive server). The user is not available to answer \
questions during this task: make sensible choices and explain them.

When you are done, your final message is your reply to the user: what you made (file paths), \
the important results, and anything they must know or do. After that reply, add a line \
`--- usability review ---` followed by a short, candid review of the tool's help and output \
for agents (confusions, guesses, retries, missing information, what worked well).

The user's request:

"""


def parse_task(path: Path) -> dict:
    text = path.read_text()
    meta = {}
    if text.startswith("---"):
        head, text = text[3:].split("---", 1)
        for line in head.strip().splitlines():
            k, v = line.split(":", 1)
            meta[k.strip()] = v.strip()
    prompt = text.split("## Prompt", 1)[1].split("\n## ", 1)[0].strip()
    title = next((l[2:].strip() for l in text.splitlines() if l.startswith("# ")), path.stem)
    return {"name": path.stem, "title": title, "prompt": prompt, **meta}


def all_tasks() -> list:
    return [parse_task(p) for p in sorted(TASKS.glob("*.md"))]


def user_config_hash() -> str:
    """Hash of the real user config, to prove evals never touch it."""
    for p in [Path.home() / ".config/ldraw2print/config.toml", REPO / "ldraw2print.toml"]:
        if p.exists():
            return hashlib.sha256(p.read_bytes()).hexdigest()
    return "absent"


def library_dir() -> str:
    for cand in [os.environ.get("EVAL_LDRAWDIR"), os.environ.get("LDRAWDIR"), str(Path.home() / "ldraw")]:
        if cand and (Path(cand) / "parts").is_dir():
            return cand
    sys.exit("No LDraw library found: set EVAL_LDRAWDIR (needed for tasks with `library: yes`).")


def setup_sandbox(task: dict, root: Path) -> Path:
    d = root / task["name"]
    (d / "work").mkdir(parents=True)
    (d / "home").mkdir()
    (d / "bin").mkdir()
    cfg = task.get("config", "none")
    if cfg != "none":
        dest = d / "home/.config/ldraw2print/config.toml"
        dest.parent.mkdir(parents=True)
        shutil.copy(EVALS / "fixtures" / f"config-{cfg}.toml", dest)
    for f in filter(None, (x.strip() for x in task.get("files", "").split(","))):
        shutil.copy(EVALS / f, d / "work" / Path(f).name)
    # The agent's own `export LDRAWDIR=…` / --ldraw-dir always win; for
    # `library: yes` the library is only the default.
    lib = f': "${{LDRAWDIR:={library_dir()}}}"; export LDRAWDIR' if task.get("library", "yes") == "yes" else "# no library configured"
    wrapper = d / "bin/ldraw2print"
    wrapper.write_text(
        "#!/bin/sh\n"
        "# Eval sandbox: the tool sees only this task's home/config and library setting.\n"
        f'export HOME="{d / "home"}"\n'
        f'export XDG_CONFIG_HOME="{d / "home/.config"}"\n'
        "unset LDRAW2PRINT_CONFIG\n"
        f"{lib}\n"
        f'exec "{TOOL}" "$@"\n'
    )
    wrapper.chmod(0o755)
    return d


def run_task(task: dict, root: Path, model: str | None, timeout: int) -> dict:
    d = setup_sandbox(task, root)
    env = dict(os.environ)
    env["PATH"] = f"{d / 'bin'}:{env['PATH']}"
    env.pop("LDRAWDIR", None)
    cmd = [
        "claude", "-p", PREAMBLE + task["prompt"],
        "--output-format", "stream-json", "--verbose",
        "--no-session-persistence",
        "--restricted", "--tools", "Bash,Read,Write,Edit,Glob,Grep",
        "--allowedTools", "Bash,Read,Write,Edit,Glob,Grep",
        "--permission-mode", "dontAsk",
        "--strict-mcp-config",
    ]
    if model:
        cmd += ["--model", model]
    t0 = time.time()
    with open(d / "transcript.jsonl", "w") as out, open(d / "stderr.txt", "w") as err:
        try:
            p = subprocess.run(cmd, cwd=d / "work", env=env, stdout=out, stderr=err, timeout=timeout)
            status = p.returncode
        except subprocess.TimeoutExpired:
            status = "timeout"
    report = ""
    for line in (d / "transcript.jsonl").read_text().splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "result":
            report = ev.get("result", "") or ""
    (d / "report.md").write_text(report)
    return {"task": task["name"], "status": status, "seconds": round(time.time() - t0)}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tasks", nargs="*", help="task names or prefixes (default: all)")
    ap.add_argument("--parallel", type=int, default=3)
    ap.add_argument("--model", help="claude model alias (default: claude's default)")
    ap.add_argument("--timeout", type=int, default=1200, help="seconds per task")
    ap.add_argument("--check", metavar="RUN_DIR", help="only re-run checks on an existing run")
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()

    tasks = all_tasks()
    if args.list:
        for t in tasks:
            print(f"{t['name']:<24} {t['title']}")
        return
    if args.tasks:
        tasks = [t for t in tasks if any(t["name"] == s or t["name"].startswith(s) for s in args.tasks)]
        if not tasks:
            sys.exit("no matching tasks (see --list)")

    import checks  # noqa: E402  (same directory)

    if args.check:
        root = Path(args.check).resolve()
        meta = json.loads((root / "meta.json").read_text())
        results = checks.check_run(root, [t for t in tasks if (root / t["name"]).is_dir()], meta)
        checks.print_summary(results)
        return

    if not TOOL.exists():
        sys.exit(f"build the tool first: cargo build --release ({TOOL} missing)")
    root = RUNS / dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    root.mkdir(parents=True)
    meta = {
        "started": dt.datetime.now().isoformat(timespec="seconds"),
        "tool_version": subprocess.run([str(TOOL), "--version"], capture_output=True, text=True).stdout.strip(),
        "git": subprocess.run(["git", "-C", str(REPO), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip() or "uncommitted",
        "model": args.model or "default",
        "user_config_hash_before": user_config_hash(),
        "tasks": [t["name"] for t in tasks],
    }
    print(f"Running {len(tasks)} task(s) in {root} ({args.parallel} in parallel)…", flush=True)
    runs = []
    with cf.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        futs = {pool.submit(run_task, t, root, args.model, args.timeout): t for t in tasks}
        for f in cf.as_completed(futs):
            r = f.result()
            runs.append(r)
            print(f"  finished {r['task']} ({r['seconds']} s, exit {r['status']})", flush=True)
    meta["runs"] = sorted(runs, key=lambda r: r["task"])
    meta["user_config_hash_after"] = user_config_hash()
    (root / "meta.json").write_text(json.dumps(meta, indent=2))
    results = checks.check_run(root, tasks, meta)
    checks.print_summary(results)
    print(f"\nTranscripts, reports and check details: {root}")


if __name__ == "__main__":
    sys.path.insert(0, str(EVALS))
    main()
