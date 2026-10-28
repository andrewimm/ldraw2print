---
library: yes
config: default
---
# Pirate captain's hat (multicolour, rib grips)

Regression for the bicorne path: part discovery by theme, head-body
connector, `--close-open-shells` (ribs vanish without it), painted 3MF.

## Prompt

I want a 5:1 scale pirate captain's hat for my minifigure, the classic black
one with a white skull and crossbones, as a multicolour file for my Bambu
printer. Use my usual head fit.

## Good outcome

- Picks a 2528ap3x bicorne (ideally 2528ap31, the 1989 classic; 2528ap30 is
  acceptable if the agent explains the choice).
- `--color 0`, `--connector head-body`, `--close-open-shells`, `.3mf` output.
- Tells the user the filament order (black base, white painted).
- Says the fit is not calibrated (head-body has no calibration record).

## Automatic checks

A `.3mf` in work/; a convert command with 2528ap3, --color 0, head-body and
--close-open-shells; the report mentions filaments and that the fit isn't
verified/calibrated.
