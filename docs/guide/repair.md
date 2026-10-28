# Diagnostics and repair

LDraw is made for rendering, so many parts aren't closed solids. Every
build reports what's wrong instead of pretending it's fine.

Watertight = closed, manifold, consistently wound, positive volume, no
collapsed triangles.

| Issue code | Severity | Meaning |
|------------|----------|---------|
| open-boundary | error | holes: edges used by one triangle |
| non-manifold | error | edges shared by more than two triangles |
| inconsistent-winding | error | neighbouring faces disagree on inside/outside |
| volume | error | closed but inverted or flat |
| open-shells | warning | separate open pieces slicers may DROP (ribs, bosses) |
| intersections | warning | overlapping shells (slicers usually union them) |
| unknown-orientation | warning | non-BFC geometry; facing guessed |
| degenerate | warning | collapsed triangles |
| needles / t-junctions / components | info | usually harmless |

`inspect PART --problems 50` lists open edges with coordinates and the LDraw
file they come from.

Repairs (opt-in; none moves an existing vertex):

- `--close-open-shells`: closes separate open pieces (caps flat openings,
  completes convex pieces such as ribs) and anchors them into the body so
  the slicer fuses them. Use it whenever `open-shells` is reported. Without
  it, bicorne 2528's stud-gripping ribs vanish from the slice.
- `--fix-t-junctions`: splits faces where another vertex sits on an edge.
- `--orient`: infers facing for non-certified geometry.
- Default cleanup removes collapsed and exact-duplicate triangles, and
  needles only where removal can't open the mesh (`--no-cleanup` skips).

Not done on purpose: hole filling of the main body, boolean union,
remeshing; they could change connector fit.

Known authored defects: top hat 3878 has a doubled inner wall that slicers
turn into a solid underside; use the corrected copy examples/3878-print.dat
(in the source repository). It still has 32 open edges, a 0.16-LDU mismatch
on an internal ceiling that slices cleanly, so don't use `--strict` with it.
`--strict` makes convert exit 3 and write nothing unless watertight.
