# Slicers

All output is millimetres, Z up, already on the bed. Import as-is; never
scale in the slicer (change --scale so fits are recalculated).

Bambu Studio (verified)
- File → Import. STL and painted 3MF both load at the right size.
- Painted 3MF: filament slots follow the printed order.
- `BambuStudio --info file.stl` prints size and manifold status. It also
  writes a `result.json` into the current directory; run it from a scratch
  folder.

OrcaSlicer
- Bambu Studio fork; same workflow. Painted 3MF should work (untested).
- Its "X-Y hole compensation" affects every hole; use it or the tool's
  per-connector fit, not both.

PrusaSlicer
- Reads `slic3rpe:mmu_segmentation` paint (untested).
- "XY Size Compensation" changes the whole outline; leave at 0 and use the
  tool's connector fit.
- Organic supports; support blockers keep supports out of sockets.

Cura
- Turn off "Scale large models" / "Scale extremely small models".
- Ignores painted colour; "Hole Horizontal Expansion" affects all holes.

Everywhere: don't compensate twice (slicer vs `[printer]` compensation),
calibrate with the profile you'll print with, and turn off auto-orientation.
