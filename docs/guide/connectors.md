# Connectors

A connector profile describes how an accessory attaches to a minifigure:

    ldraw2print profiles            list them with sizes at your scale

| Profile   | Kind   | Nominal | Mates with |
|-----------|--------|---------|------------|
| head-stud | socket | Ø12 LDU | head top stud (hats, hair, helmets) |
| head-body | socket | Ø26 LDU | the head itself (recess of e.g. bicorne 2528); default fit loose |
| neck      | socket | Ø12 LDU | torso neck post (backpacks, airtanks; C-clips allowed) |
| scarf     | socket | Ø16 LDU | head's bottom collar (scarves 25376, 24504) |
| hand-grip | pin    | Ø8 LDU  | minifig hand (spears, tools) |

Nominal sizes come from LDraw geometry of the mating part; they are not
measurements of any printed figure.

## Detection

`--connector NAME` finds cylinder primitives (n-4cyli) in the part, merges
coaxial pieces into candidates and keeps those that match the profile:
facing (inward for sockets, outward for pins), axis (vertical for head and
neck profiles), diameter within `match_tolerance` of nominal, and enough of a
full circle (`min_coverage`).

- Exactly one match → used automatically ("auto").
- None or several → `convert`/`calibrate` stop with **exit code 4** and list
  the candidates; `inspect` succeeds and reports `selection_error`.

Resolve exit code 4 by choosing a candidate id from the listing:

    ldraw2print inspect 3959 --connector hand-grip      shows candidates #0..#5
    ldraw2print convert 3959 --connector hand-grip --connector-candidate 3 --fit clearance

or, for a part whose connector isn't built from cylinder primitives, describe
it (LDraw coordinates, LDU):

    --connector-axis 0,0,0 --connector-radius 6 [--connector-dir 0,-1,0] [--connector-range 0,4]

  - `--connector-axis`: a point on the axis (LDraw coordinates, LDU).
  - `--connector-dir`: axis direction; default 0,-1,0 is "up" in LDraw.
  - `--connector-range a,b`: distances ALONG that direction from the axis
    point (so a socket rising 4 LDU from the origin is 0,4). Omit it to use
    every vertex on the cylinder.
  - Pass the same manual options to `convert` as to `inspect`.

## What --fit changes

Only vertices of the selected surface's own faces move, radially, to reach
the target diameter. Everything else, including geometry that merely touches
the circle, stays where it is. Without `--fit` the connector is exported as
modelled. If the wall is too thin for the change, the fit is refused.

## Limits

- Only round (cylindrical) connectors can be detected and fitted. Rib grips
  (e.g. the four ribs inside bicorne 2528) are kept as modelled.
- Detection matches geometry, not meaning: `head-stud` on the head 3626c
  finds the head's own neck hole. Check the selected surface.
