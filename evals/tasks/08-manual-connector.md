---
library: yes
config: default
files: fixtures/myhat.dat
---
# Custom hat file whose socket isn't built from primitives

Tests the manual connector path (`--connector-axis/--connector-radius`).

## Prompt

I made my own LDraw hat file, myhat.dat (it's in the current folder). Please
convert it at 5:1 with a normal fit on the head stud socket.

## Good outcome

- Runs convert/inspect with `--connector head-stud`, gets exit 4 / no
  candidates, reads `guide connectors`, and describes the socket manually:
  `--connector-axis 0,0,0 --connector-radius 6` (the socket is r = 6 LDU
  around the origin, opening at y = 0).
- Output written with the fit applied (32 vertices moved).

## Automatic checks

A convert/inspect command with --connector-axis and --connector-radius 6;
an output file; the transcript shows the fit was applied.
