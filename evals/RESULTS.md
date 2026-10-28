# Eval results log

Newest first. Record the date, tool state, model and anything learned.
Full transcripts stay in `runs/` (not committed).

## 2026-10-27, run 20261027-163148: 3/3 pass (01, 03, 06)

After adding `calibrate --record` and the new search ranking.

- 06: the agent found `--record` in the help and used it on its first try.
  Nobody hand-edited the TOML. It noted on its own that it assumed the
  default coupon set. The config diff is the single calibration line.
- 01: `search pirate hat` / `search bicorne` led straight to the bicorne.

## 2026-10-27, run 20261027-161819: 8/8 pass

Model: Claude Code default. After the fixes below.

- 04-first-run: the agent found the unconfigured library, used
  `--ldraw-dir`, and told the user how to set `LDRAWDIR` / `ldraw_dir`. This
  follows the new `AGENTS.md` guidance; the previous run silently did not.
- 03-torch: tried `--smooth 96`, saw the seams warning, rebuilt without it.
- 06-calibration-loop: added a well-formed `calibration` record. The user's
  real config was unchanged (hash checked).
- 07-out-of-scope: stated the limit and exported the parts separately, with
  joint fit flagged as untested. (Manual review: good.)

## 2026-10-27, run 20261027-160500: 7/8 (re-graded)

First full run. The agents' usability reviews found these, all fixed the
same day:

- A fit was refused on the construction helmet with `--smooth 96
  --close-open-shells`. Cap triangulation made slivers, and the
  wall-thickness test counted sliver flips. Fixed: best-shaped ear clipping,
  plus a "does the moving surface pass through unmoved geometry" test.
- `default_fit_source` said "built-in default" despite a calibration record;
  the nominal-size warning persisted after calibration.
- "main colour not chosen" was shown when no triangle used the main colour.
- `--problems` didn't list inconsistent-winding edges.
- The no-candidates error suggested `--connector-candidate`.
- `library` didn't say where it looked.
- `volume_cm3: null` had no explanation (`volume_estimate_cm3` added).
- Search put keyword-only matches ("Carrot") above name matches.
- Wrong `--connector-range` example in the connectors guide (found in the
  first smoke run).
- Docs: scope statement, head-stud vs head-body, PrusaSlicer extruder
  mapping, multi-toolhead printers, heads orientation, hand-written .ldr.
- Harness: the wrapper overrode the agent's own `LDRAWDIR`; 04 is now graded
  on whether the agent found a library on disk.

Feature requests deferred (raised by agents):

- ~~`calibrate --record N --printer … --material …` to write the config entry.~~ Done.
- Ring/cylinder detection in raw (non-primitive) parts, to suggest a manual
  connector axis.
- A shape description or thumbnail, so agents can tell which part they have.
- A print time / filament estimate (needs a slicer).
- A compact JSON mode for agents with small context windows.
- ~~Better search ranking for generic queries ("minifig head" → 3626c).~~ Done.
