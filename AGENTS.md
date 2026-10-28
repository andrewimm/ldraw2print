# ldraw2print for agents

ldraw2print turns LEGO LDraw parts into 3D-printable STL / multicolour 3MF,
default 5:1 scale, millimetres, Z up. This file is also `ldraw2print guide agents`.

## Scope

One LDraw part (or an assembled .ldr/.mpd model fused into one solid) →
STL / painted 3MF, with fits for minifigure connections. No print-in-place
joints, articulated figures, or arranging multiple parts on a plate. Say so
plainly when a request needs those.

## Rules

1. Use `--json` (global flag) and parse stdout. One JSON object per run.
2. Branch on the exit code, not on text:

   | Code | Meaning | What to do |
   |------|---------|------------|
   | 0 | ok | continue |
   | 1 | error (bad input, part not found, I/O, …) | read `error`; fix input |
   | 2 | command-line usage error | fix flags (`<command> --help`) |
   | 3 | `--strict` and the mesh isn't watertight; nothing written | inspect, repair or drop `--strict` |
   | 4 | connector needs a decision (no or several matching surfaces) | pick an id from the candidates, rerun with `--connector-candidate ID` |
   | 5 | referenced LDraw files missing | fix the library path; `--allow-missing` only if a partial part is acceptable |

3. Never run `preview` unattended: it is a server that runs until killed
   and opens a browser. Use `inspect --json` / `convert` (same pipeline).
4. Don't invent part numbers or colour codes: `search` and `colors` find them.
5. Don't claim a fit works. Nominal sizes are LDraw geometry; real fit needs
   `calibrate` coupons printed on the user's printer.
6. Report what the tool reports: `watertight`, `issues`, `warnings`. A
   written file is not proof of printability.

## Discovery

    ldraw2print --json library                 is the LDraw library installed? where?
    ldraw2print --json search pirate bicorne   part numbers from words (all must match)
    ldraw2print --json colors black            colour codes (use with --color)
    ldraw2print --json profiles                connector profiles + the user's calibrated fits
    ldraw2print --json config show             effective configuration
    ldraw2print guide                          topic list; ldraw2print guide <topic>

If `library` has no `path`, the tool isn't configured: `searched` lists
where it looked.
- If you can find an installed library yourself (e.g. a folder with
  `parts/` and `p/`), use `--ldraw-dir PATH` and TELL the user: say where you
  found it and how to make it permanent (`export LDRAWDIR=PATH` or
  `ldraw_dir = "PATH"` in the config). Otherwise the next session hits the
  same problem.
- If there's none, the user must install it (the JSON includes `install`
  instructions). Don't download the 150 MB library without asking.
Explicit .dat/.ldr/.mpd file paths work without a library.

## Recipes

`--smooth 96` rounds LDraw's 16-sided curves and is usually worth it, but on
parts with axle holes or chord pieces it can open seams. Check
`smoothing_new_seams_mm` (0 = fine) / `verdict.reasons`; if it opened seams,
build that part without `--smooth`.

Single-colour accessory with a calibrated head-stud fit:

    ldraw2print --json convert 3624 --connector head-stud --fit clearance --smooth 96 -o hat.stl

Printed (multicolour) part, painted 3MF; tell the user the filament order
from the `filaments` array:

    ldraw2print --json convert 3626cp01 --color 14 --smooth 96 -o head.3mf

Hat that slides over the head and has rib grips (bicorne family; 2528ap31 is
the 1989 Pirates captain's print, 2528ap30 the later "evil skull"):

    ldraw2print --json convert 2528ap30 --color 0 --smooth 96 \
      --connector head-body --fit clearance --close-open-shells -o bicorne.3mf

Calibration coupons (always before trusting a fit on a new printer/material):

    ldraw2print --json calibrate --connector head-stud -o coupons.stl
    # user tested them: coupon 2 fit best
    ldraw2print --json calibrate --connector head-stud --record 2 --printer "Prusa MK4" --material PLA

Record results with `--record`, not by editing the config: it writes the
right file, keeps comments, and stores the exact clearance. Ask the user
for printer and material if you don't know them.

Resolve "decision needed" (exit 4):

    ldraw2print --json inspect 3959 --connector hand-grip     → connector.candidates[]
    ldraw2print --json convert 3959 --connector hand-grip --connector-candidate 3 --fit clearance

Choosing among candidates is about how the part is used, which geometry
alone can't tell: compare `diameter_ldu`, `depth_ldu` and `axis_dir`, and
the part's description. On torch 3959, #3 is the short perpendicular grip
stub and #4/#5 are the barrel. If it isn't clear, ask the user.

## Decide flags from inspect

Run `ldraw2print --json inspect PART [--connector P]` first and check:

- `verdict.status`: "ready" or "review"; `verdict.reasons` say what to do
  (e.g. add `--close-open-shells`). Rerun inspect with the fix until ready,
  or explain the remaining reasons to the user.
- Without `--connector`: `connector_suggestions` lists profiles whose shape
  matches. Geometric only: a hat's plume post matches `hand-grip`. Pick the
  one that matches how the part attaches (see `guide connectors`).
- `issues[].code == "open-shells"` → add `--close-open-shells`.
- `watertight == false` → look at `issues`; `inspect --problems 50` shows
  where. Many official parts are open but slice fine; say so plainly.
- `connector.selected == null` → see `selection_error` and `candidates`.
- `connector.plan.mating_source == "nominal"` → fit is unverified; suggest
  measuring the figure (`measured_diameter_mm`) or calibrating.
- `colors` has more than one entry → a `.3mf` output gives multicolour.
  `main_color.source == "default"` means you haven't chosen the base colour
  (default 71, grey): pass `--color` with the colour the user wants.
- `fit_summary.status` tells you how much to trust a fit:
  "measured+calibrated" is verified for this figure; "calibrated-preset-nominal-size"
  means the user's tested clearance applied to the nominal stud size (usually
  fine, say it's nominal); "unverified" means nothing was tested: recommend
  `calibrate` first. `calibrated` means a calibration record exists in the
  config (`profiles` shows its printer, material and scale). If the record's
  printer, material or scale differs from the job, say the calibration may
  not transfer (a scale mismatch is also in `warnings`).
- Size: `size_mm`; anything over the printer's bed needs a smaller `--scale`.

## Flags that change geometry

| Flag | Effect |
|------|--------|
| `--fit F` / `--clearance-mm X` | moves only the selected connector surface radially |
| `--smooth N` | rounder curves (N multiple of 16; 96 recommended) |
| `--close-open-shells` | closes separate open pieces and fuses them to the body |
| `--flip` | prints upside down (rotation only) |
| `--fix-t-junctions`, `--orient` | topology-only repairs |

Without these, geometry is exported as modelled (plus safe cleanup).

## Pitfalls

- `--fit` without `--connector` is a usage error (exit 2).
- `--fit clearance` means "the profile's default fit". That is the user's
  calibration only if the profile is `calibrated`; otherwise it's the
  shipped default.
- Hats: `head-stud` for hats that grip the head's top stud with a round
  socket; `head-body` for hats that slide over the head (bicorne 2528
  family). `connector_suggestions` tells you which shapes the part has.
- `inspect` with an unresolved connector exits 0 (with `selection_error`);
  `convert`/`calibrate` exit 4.
- `head-stud` on a head part matches the head's neck hole; check
  `connector.selected` is what you meant.
- Orientation: the tool doesn't choose it. `guide orientation` has the
  method; `placement.flip` in the JSON records what you chose.
- `verdict.reasons` can remain after `--close-open-shells` (flat sheets or
  covered openings can't be closed safely); that's reported, not hidden.
- Top hat 3878 has an authored defect: convert `examples/3878-print.dat` (if
  present) or expect a filled underside. That copy still reports 32 open
  edges; they slice cleanly, so "review" is expected there.
- Part names can be ambiguous ("torch" is a flashlight in LDraw's British
  English; 13793 is a welding torch). The tool can't show you the shape;
  confirm with the user when the description could mean two things.
- Coordinates: connector candidates are in LDraw units and LDraw axes;
  `problem_edges` and `size_mm` are print-frame millimetres after placement.
- Painted 3MF works in Bambu Studio / OrcaSlicer / PrusaSlicer, not Cura.
- Colour changes are expensive on single-nozzle printers (bicorne: 92 swaps)
  but cheap on multi-toolhead printers (Prusa XL, toolchangers).
- `main_color.triangles == 0` means every triangle has its own colour;
  `--color` then changes nothing.
- Hand-written .ldr files: start with `0 BFC CERTIFY CCW` (otherwise facing
  is guessed) and give each line an explicit colour.

More: `ldraw2print guide json` for every JSON field.
