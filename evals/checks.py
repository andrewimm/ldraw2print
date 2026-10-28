"""Automatic checks for ldraw2print agent evals (see evals/README.md).

Every check returns (status, message) with status "pass", "fail" or
"warn". A task passes when none of its checks fail. Checks look at the
agent's commands (from the transcript), its final report, the files it
wrote in work/, and, for some tasks, the sandbox state after the run.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path


# ---------------------------------------------------------------------------
# Transcript helpers


class Run:
    def __init__(self, d: Path):
        self.dir = d
        self.work = d / "work"
        self.report = (d / "report.md").read_text() if (d / "report.md").exists() else ""
        self.reply = self.report.split("--- usability review ---")[0]
        self.commands: list[str] = []
        self.outputs: list[str] = []
        self.reads: list[str] = []
        self.events = []
        for line in (d / "transcript.jsonl").read_text().splitlines() if (d / "transcript.jsonl").exists() else []:
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            self.events.append(ev)
            msg = ev.get("message") or {}
            if not isinstance(msg, dict):
                continue
            for part in msg.get("content", []) if isinstance(msg.get("content"), list) else []:
                if not isinstance(part, dict):
                    continue
                if part.get("type") == "tool_use":
                    inp = part.get("input", {})
                    if part.get("name") == "Bash":
                        self.commands.append(inp.get("command", ""))
                    if part.get("name") in ("Read", "Grep", "Glob"):
                        self.reads.append(json.dumps(inp))
                if part.get("type") == "tool_result":
                    c = part.get("content")
                    if isinstance(c, list):
                        c = "\n".join(x.get("text", "") for x in c if isinstance(x, dict))
                    self.outputs.append(str(c))

    def ran(self, *needles: str) -> list[str]:
        """Commands containing all needles (regex)."""
        return [c for c in self.commands if all(re.search(n, c) for n in needles)]

    def converts(self) -> list[str]:
        return self.ran(r"ldraw2print\b.*\bconvert\b")

    def files(self, *exts: str) -> list[Path]:
        return [p for p in self.work.rglob("*") if p.is_file() and p.suffix.lower() in exts]

    def said(self, pattern: str) -> bool:
        return re.search(pattern, self.reply, re.I | re.S) is not None

    def output_has(self, pattern: str) -> bool:
        return any(re.search(pattern, o, re.S) for o in self.outputs)


def ok(cond: bool, msg: str, fail: str = "fail") -> tuple[str, str]:
    return ("pass" if cond else fail, msg)


# ---------------------------------------------------------------------------
# Checks applied to every task


def common(r: Run, meta: dict) -> list:
    out = [
        ok(bool(r.reply.strip()), "final reply to the user is present"),
        ok(not r.ran(r"ldraw2print\b.*\bpreview\b"), "never ran the interactive preview"),
        ok(
            not any(re.search(r"/src/\S*\.rs|\.rs\b|README\.md", x) for x in r.reads + r.commands),
            "did not read the tool's source code or README",
            "warn",
        ),
        ok(
            meta.get("user_config_hash_before") == meta.get("user_config_hash_after"),
            "the user's real config was not modified",
        ),
        ok("--- usability review ---" in r.report, "included a usability review", "warn"),
    ]
    return out


# ---------------------------------------------------------------------------
# Per-task checks


def t01_pirate_hat(r: Run, meta: dict) -> list:
    conv = r.ran(r"\bconvert\b", r"2528ap3")
    return [
        ok(bool(r.files(".3mf")), "wrote a .3mf (multicolour)"),
        ok(bool(conv), "converted a 2528ap3x bicorne"),
        ok(any("--color 0" in c or "--color=0" in c for c in conv), "black base colour (--color 0)"),
        ok(any("head-body" in c for c in conv), "used the head-body connector"),
        ok(any("--close-open-shells" in c for c in conv), "closed the open rib shells (--close-open-shells)"),
        ok(r.said(r"filament"), "told the user the filament order"),
        ok(r.said(r"not (been )?calibrat|uncalibrat|no calibration|unverified|not verified|nominal"), "said the fit isn't calibrated/verified"),
        ok(any("2528ap31" in c for c in conv) or r.said(r"2528ap31|1989|classic"), "picked or discussed the 1989 classic print (2528ap31)", "warn"),
    ]


def t02_construction_helmet(r: Run, meta: dict) -> list:
    conv = r.ran(r"\bconvert\b", r"3833")
    return [
        ok(bool(r.files(".stl", ".3mf")), "wrote an output file"),
        ok(bool(conv), "converted part 3833"),
        ok(any(re.search(r"--color[ =]14\b", c) for c in conv), "yellow (--color 14)"),
        ok(any("head-stud" in c for c in conv), "used the head-stud connector"),
        ok(any("--fit" in c for c in conv), "applied a fit"),
        ok(r.said(r"calibrat"), "discussed the calibration status"),
        ok(r.said(r"nominal|not measured|unverified|not verified"), "said the stud size is nominal / not verified"),
    ]


def t03_torch(r: Run, meta: dict) -> list:
    conv = r.ran(r"\bconvert\b", r"3959|86208")
    # Judge the conversion that produced the final file (agents may try
    # --smooth, see the seams warning and rebuild without it: that's good).
    smooth = bool(conv) and "--smooth" in conv[-1]
    return [
        ok(bool(r.files(".stl", ".3mf")), "wrote an output file"),
        ok(bool(conv), "converted a handheld torch (3959/86208)"),
        ok(any(re.search(r"--color[ =]70\b", c) for c in conv), "reddish brown (--color 70)"),
        ok(any("--connector-candidate" in c for c in conv) or not any("hand-grip" in c for c in conv), "resolved the candidate choice"),
        ok(any(re.search(r"--connector-candidate[ =]3\b", c) for c in r.ran(r"3959")), "chose the grip stub (#3) on 3959", "warn"),
        ok(not smooth or r.said(r"seam"), "no --smooth, or reported the seams it opened"),
        ok(r.said(r"calibrat|coupon"), "recommended calibration for the grip"),
    ]


def t04_first_run(r: Run, meta: dict) -> list:
    downloaded = r.ran(r"(curl|wget).*complete\.zip")
    # The agent's shell sees the real machine: if a library exists on disk it
    # may find it and point the tool at it. That's the "installed but not
    # configured" variant: fine, as long as it tells the user how to configure.
    found = [c for c in r.ran(r"--ldraw-dir|LDRAWDIR=") if "ldraw" in c.lower()]
    if found and r.files(".stl", ".3mf"):
        return [
            ("pass", "variant: found an existing LDraw library the tool wasn't configured for"),
            ok(r.said(r"LDRAWDIR|ldraw_dir"), "told the user how to configure the library location (LDRAWDIR / ldraw_dir)"),
            ok(r.said(r"found|already (installed|have)|existing|located"), "told the user it found their library", "warn"),
            ok(not downloaded, "did not download the 150 MB library unasked", "warn"),
        ]
    return [
        ok(not r.files(".stl", ".3mf"), "did not produce a model without the library"),
        ok(r.said(r"ldraw.*librar|librar.*ldraw"), "explained the LDraw library is needed"),
        ok(r.said(r"library\.ldraw\.org|complete\.zip"), "said where to get it"),
        ok(r.said(r"LDRAWDIR|--ldraw-dir|~/ldraw|ldraw_dir"), "said how to point the tool at it"),
        ok(not downloaded, "did not download the 150 MB library unasked", "warn"),
    ]


def t05_prusa_head(r: Run, meta: dict) -> list:
    conv = r.ran(r"\bconvert\b", r"3626")
    return [
        ok(bool(r.files(".3mf")), "wrote a .3mf"),
        ok(bool(conv), "converted a 3626 head"),
        ok(any(re.search(r"--color[ =]14\b", c) for c in conv), "yellow base (--color 14)"),
        ok(r.said(r"prusaslicer"), "addressed PrusaSlicer"),
        ok(r.said(r"untested|unverified|(not|only) (been |yet )?(tested|verified)|(tested|verified) (only )?(in|with) bambu"), "said PrusaSlicer paint support is unverified"),
        ok(r.said(r"filament|extruder|tool ?head"), "explained how to assign the two colours"),
    ]


def sandbox_profiles(r: Run) -> dict | None:
    wrapper = r.dir / "bin" / "ldraw2print"
    p = subprocess.run([str(wrapper), "--json", "profiles"], capture_output=True, text=True)
    if p.returncode != 0:
        return None
    return json.loads(p.stdout)


def t06_calibration_loop(r: Run, meta: dict) -> list:
    prof = sandbox_profiles(r)
    hs = next((p for p in (prof or {}).get("profiles", []) if p["name"] == "head-stud"), None) if prof else None
    rec = (hs or {}).get("calibration") or {}
    conv = r.ran(r"\bconvert\b", r"3624")
    return [
        ok(prof is not None, "the edited config still loads"),
        ok(bool(hs and hs.get("calibrated")), "head-stud now has a calibration record"),
        ok(bool(hs and hs.get("default_fit") == "normal"), "default_fit set to normal (coupon 2)"),
        ok(rec.get("scale") == 5.0, "record scale is 5"),
        ok("pla" in str(rec.get("material", "")).lower(), "record material mentions PLA"),
        ok(re.search(r"prusa|mk4", str(rec.get("printer", "")), re.I) is not None, "record printer mentions Prusa/MK4"),
        ok(bool(r.ran(r"calibrate\b.*--record[ =]2\b")), "recorded with `calibrate --record 2` (not hand-edited)", "warn"),
        ok(bool(r.ran(r"ldraw2print\b.*\bprofiles\b")), "verified with `profiles`", "warn"),
        ok(bool(conv) and bool(r.files(".stl", ".3mf")), "converted the police hat (3624)"),
        ok(any(re.search(r"--fit[ =](clearance|normal)\b", c) for c in conv), "used the recorded fit"),
    ]


def t07_out_of_scope(r: Run, meta: dict) -> list:
    return [
        ok(r.said(r"(can'?t|cannot|doesn'?t|does not|won'?t|not able|no support|isn'?t).{0,200}(joint|articulat|mov|pose)|(joint|articulat|mov|pose).{0,200}(can'?t|cannot|not|isn'?t|aren'?t)"), "stated the limitation about moving joints"),
        ok(not r.said(r"(arms|legs|joints) (will|can) (now )?move|fully articulated (print|model) ready"), "did not claim working joints"),
        ("warn", "manual review recommended: judge the alternative offered"),
    ]


def t08_manual_connector(r: Run, meta: dict) -> list:
    manual = r.ran(r"--connector-axis", r"--connector-radius[ =]6(\.0+)?\b")
    return [
        ok(bool(manual), "described the socket manually (--connector-axis, radius 6)"),
        ok(bool(r.files(".stl", ".3mf")), "wrote an output file"),
        ok(r.output_has(r'"moved_vertices"\s*:\s*[1-9]|Applied\s+moved [1-9]'), "the fit was actually applied"),
    ]


CHECKS = {
    "01-pirate-hat": t01_pirate_hat,
    "02-construction-helmet": t02_construction_helmet,
    "03-torch": t03_torch,
    "04-first-run": t04_first_run,
    "05-prusa-head": t05_prusa_head,
    "06-calibration-loop": t06_calibration_loop,
    "07-out-of-scope": t07_out_of_scope,
    "08-manual-connector": t08_manual_connector,
}


# ---------------------------------------------------------------------------


def check_run(root: Path, tasks: list, meta: dict) -> list:
    results = []
    for t in tasks:
        d = root / t["name"]
        if not d.is_dir():
            continue
        r = Run(d)
        items = common(r, meta) + CHECKS.get(t["name"], lambda *_: [("warn", "no task-specific checks")])(r, meta)
        status = "fail" if any(s == "fail" for s, _ in items) else "pass"
        res = {
            "task": t["name"],
            "status": status,
            "checks": [{"status": s, "check": m} for s, m in items],
            "commands": len(r.commands),
            "tool_errors": sum(1 for o in r.outputs if re.search(r'"ok": false|^error:', o, re.M)),
        }
        (d / "checks.json").write_text(json.dumps(res, indent=2))
        results.append(res)
    (root / "results.json").write_text(json.dumps(results, indent=2))
    return results


def print_summary(results: list) -> None:
    print()
    for res in results:
        fails = [c for c in res["checks"] if c["status"] == "fail"]
        warns = [c for c in res["checks"] if c["status"] == "warn"]
        print(f"{res['status'].upper():<5} {res['task']:<24} {len(res['checks']) - len(fails) - len(warns)} pass, {len(warns)} warn, {len(fails)} fail, {res['commands']} commands")
        for c in fails + warns:
            print(f"        {c['status']}: {c['check']}")
    passed = sum(1 for r in results if r["status"] == "pass")
    print(f"\n{passed}/{len(results)} tasks passed")
