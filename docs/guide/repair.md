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

Layer check: open edges only matter where they break a layer's outline.
Every build of a non-watertight mesh cuts it like a slicer (0.2 mm layers,
chain ends up to 2 mm apart joined, as PrusaSlicer, Bambu Studio and
OrcaSlicer do) and reports layers whose outlines stay open, i.e. areas the
slicer silently leaves out:

    Layers: 6 of 120 layers have open outlines a slicer drops: z 8.6–9.8 mm (up to 777 mm² per layer)

`inspect --layers` lists them; `--layer-height` / `--closing-radius` change
the assumptions. Open outlines inside solid material, and slivers under
1 mm wide, are ignored. Checked against Bambu Studio on 11 parts: 98% of
flagged outlines really were dropped. When every layer closes, "not
watertight" becomes a note: the part slices as modelled (e.g. top hat
examples/3878-print.dat). Not covered: slicers' own extra repairs, and
very thin walls.

Known dropped regions: torch 3959 loses its barrel (even with repairs),
plant stem 37695 its leaves without `--close-open-shells`, flower 24866 a
1 mm band through the petals (prints/24866-print.dat is a fixed copy).

Repairs (opt-in; none moves an existing vertex):

- `--close-open-shells`: closes separate open pieces (caps flat openings;
  caps any opening buried inside the body, such as leaf roots inside plant
  stem 37695; completes convex pieces such as ribs) and anchors them into
  the body so the slicer fuses them. Use it whenever `open-shells` is reported. Without
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
