# Orientation and placement

Output is Z up, as worn: hats crown up with the socket facing the bed.

- `--placement bed` (default): lowest point on Z=0, centred in X/Y.
- `--placement ldraw`: keep the LDraw origin (aligning several exports).
- `--flip`: rotate 180° about X (print upside down). Geometry is unchanged.

When to flip: a hat whose inner stud tube hangs from the crown (top hat
3878) would start the tube in mid-air when printed upright, and supports
would end up inside the socket. Upside down the socket faces up and needs
nothing; only the brim needs support.

When not to: hats whose cavity is self-supporting (bicorne 2528) print
upright with tree supports; the supports inside the head space come out
from below.

Heads and caps: print upright as output (a head's neck hole down, a cap's
peak supported). The socket ceiling inside a head or cap is a short bridge;
nothing hangs in mid-air.

General method for any hat or helmet:

1. Look at the socket. If its closed end (ceiling) is at the crown and an
   inner tube or rib starts partway up, printing upright starts that feature
   in mid-air: either flip or rely on supports underneath it.
2. If unsure, slice both ways (with and without `--flip`). Bambu Studio
   warns "floating regions" when something starts in mid-air without
   support. Prefer the orientation that keeps supports out of the socket
   and needs the least support on visible surfaces.
3. `convert --json` records `placement.flip`, so you can report what you did.

Keep the connector axis vertical whenever possible: round holes printed
sideways come out slightly oval, which matters at 0.05–0.2 mm clearances.
The scarf 25376 prints as worn (with supports) for that reason.

Turn off the slicer's auto-orient / arrange-with-rotation; they undo
`--flip`. Bambu Studio may also rotate parts about Z when arranging; that's
harmless (paint and geometry move together).
