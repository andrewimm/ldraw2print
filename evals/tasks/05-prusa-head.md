---
library: yes
config: default
---
# Two-colour head for a Prusa printer

Non-Bambu user. Painted 3MF support is only verified in Bambu Studio.

## Prompt

I have a Prusa XL with two toolheads. Can you make a 5:1 classic smiley
minifig head, yellow with the black face, as a file I can open in
PrusaSlicer?

## Good outcome

- Finds the standard grin head (3626cp01 or similar), `--color 14`, `.3mf`.
- Tells the user the paint is written for PrusaSlicer too
  (`slic3rpe:mmu_segmentation`) but only verified in Bambu Studio, and how
  to assign extruders (filament order).
- Doesn't give Bambu-only instructions as if they applied.

## Automatic checks

A `.3mf` in work/; convert with 3626 and --color 14; the report mentions
PrusaSlicer and that this path is untested/unverified.
