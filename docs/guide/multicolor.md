# Multicolour (painted 3MF)

    ldraw2print convert 3626cp01 --color 14 --smooth 96 -o head.3mf

A `.3mf` output (or `--format 3mf`) writes ONE solid with the pattern
colours painted on its faces (`--color-mode paint`, the default). This is
right for LDraw printed patterns, which are coloured patches of one surface.

- The most-used colour is filament 1 (unpainted). Each other colour is
  painted as filament 2, 3, … The command prints the order (and `--json`
  returns it as `filaments`); load filaments in that order / map AMS slots.
- `--color CODE` sets the main colour (LDraw colour 16). Find codes with
  `ldraw2print colors black`. Pattern colours come from the part.
- Paint attributes: `paint_color` (Bambu Studio, OrcaSlicer) and
  `slic3rpe:mmu_segmentation` (PrusaSlicer), plus standard 3MF colours.
  Verified in Bambu Studio only. Cura ignores paint.
- The 3MF doesn't set filament colours; pick them in the slicer.
- The preview has an "Export 3MF (painted)" button.

Slicers:
- Bambu Studio / OrcaSlicer: filament slot N = paint N; map AMS slots.
- PrusaSlicer (MMU or XL): set the printer to the needed number of
  extruders first; filament N → extruder N (the base colour is extruder 1).
  Untested here; check the colours in the slicer's preview.

Cost: on a single-nozzle printer every colour change is a filament swap and
purge. Bicorne 2528ap30 at 5:1: 92 changes, 4 h 18 min, 57 g, versus
1 h 45 min, 38 g in plain black. Multi-toolhead printers (Prusa XL,
toolchangers) avoid most of that waste.

`--color-mode parts` writes one part per colour instead; only useful when
each colour is a closed solid of its own (not for printed patterns).
