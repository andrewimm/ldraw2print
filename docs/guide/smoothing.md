# Smoothing (--smooth N)

LDraw draws circles as 16-gons; at 5:1 a hat brim has 10 mm flats. Slicers
can't fix this (Bambu "arc fitting" only changes how G-code is written).

    ldraw2print convert 3878 --smooth 96 -o tophat.stl

N must be a multiple of 16; use 48, 96 or 144 (multiples of 48 also refine
the p/48 high-resolution primitives). 96 is a good default for most parts.

Check the result: where smoothed curves meet pieces that can't be smoothed
(axle holes, chord pieces), smoothing can open new seams. The build reports
this as a warning and a verdict reason ("--smooth opened N mm of new seams",
JSON `smoothing_new_seams_mm`). On such parts (e.g. torch 3959) build
without --smooth. Hats, heads and scarves tested so far add no seams.

What gets smoother

- Every circular LDraw primitive (cylinders, discs, rings, cones, tori),
  verified as a surface of revolution from its vertices.
- Printed patterns and other hand-modelled faces drawn flat on those
  surfaces (e.g. a face print on a head) are curved with them.
- Geometry that touches a smoothed curved wall (e.g. a rib edge on a cone)
  moves with it so contact is kept. Flat discs/rings never move touching
  geometry.
- Gaps between smoothed arcs and unsmoothed straight edges are closed.

What doesn't

- Hand-modelled geometry that isn't drawn on a smoothed primitive surface
  (e.g. a custom file with its own polygon socket) is left as authored.
- Shapes modelled by hand rather than from primitives (most of a scarf's
  drape, hair, the bicorne's outer shell) keep their facets. Smoothing those
  would need subdivision, which changes dimensions; it isn't offered.
- Profile resolution (e.g. a torus cross-section) stays as authored.

Connector fits use the new polygon count automatically. The report line
"smoothed N circular primitives …" lists anything left as authored.
