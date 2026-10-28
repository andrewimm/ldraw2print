# Workflow: from a part name to a print

1. Find the part

       ldraw2print search pirate bicorne
       ldraw2print search minifig head grin

   All words must match the part number, description, category or LDraw
   keywords (themes such as "pirate", BrickLink/Rebrickable ids, set numbers). Printed
   variants share a base number with a pattern suffix (3626c → 3626cp01).

2. Look at it

       ldraw2print inspect 2528ap30 --connector head-body
       ldraw2print preview 2528ap30          (interactive; runs until Ctrl+C)

   Check: size, watertight, warnings, colours, and which connector surface
   was selected. `inspect` never writes files.

3. Pick the connector profile

       ldraw2print profiles

   head-stud (hats, hair, helmets on the head's top stud), head-body (hats
   that slide over the head, e.g. bicorne), neck (backpacks, airtanks),
   scarf (scarves on the head's bottom collar), hand-grip (handheld bars).

4. Calibrate once per printer + material + profile

       ldraw2print calibrate --connector head-stud -o coupons.stl

   Print, test on the figure, then set `default_fit` for that profile in the
   config (see `ldraw2print guide fit`).

5. Convert

       ldraw2print convert 3624 --connector head-stud --fit clearance --smooth 96 -o hat.stl
       ldraw2print convert 2528ap30 --color 0 --smooth 96 \
           --connector head-body --fit clearance --close-open-shells -o bicorne.3mf

   - `--fit clearance` applies the profile's default fit (your calibration).
   - `--smooth 96` rounds 16-sided LDraw curves (see `guide smoothing`).
   - `.3mf` output paints printed patterns for multicolour (see `guide multicolor`).
   - `--close-open-shells` when inspect warns about separate open shells.
   - `--flip` for hats whose inner tube would otherwise start in mid-air
     (see `guide orientation`).

6. Slice and check the preview in the slicer, especially the connector area.

Checklist before printing

- `Watertight: yes`, or the open edges are understood (`inspect --problems 50`).
- No "separate open shells" warning (or `--close-open-shells` was used).
- The connector line shows the expected surface and "Applied … moved N vertices".
- For 3MF: load filaments in the printed order.
