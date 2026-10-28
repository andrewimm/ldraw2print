# Units, scale and files

- 1 LDU (LDraw unit) = 0.4 mm on a real brick. A stud pitch is 20 LDU.
- `--scale S` multiplies: 1 LDU = 0.4 × S mm. Default S = 5 (2 mm per LDU).
  Set the default with `default_scale` in the config.
- Output axes: X = LDraw X, Y = LDraw Z, Z = −LDraw Y (LDraw's −Y is up).
- `--connector-axis/-range/-radius` take LDraw coordinates in LDU.
- Clearances, compensation and measured diameters are millimetres and do
  NOT scale; nominal sizes (LDU) do.

LDraw library (needed for part numbers; explicit .dat/.ldr paths work
without it): `--ldraw-dir`, then `$LDRAWDIR`, then `ldraw_dir` in the config,
then auto-detected ~/ldraw and similar. `ldraw2print library` shows which
one is used and how to install it.

Config file: `--config FILE`, `$LDRAW2PRINT_CONFIG`, ./ldraw2print.toml,
~/.config/ldraw2print/config.toml (first found). `ldraw2print config init`
writes a commented default; `config show` prints the effective settings
(built-in profiles merged with yours); `config path` lists the locations.

Hand-written .ldr models: start the file with `0 BFC CERTIFY CCW` and give
every line an explicit colour code; reference parts by number
(`1 <colour> x y z a b c d e f g h i 3626c.dat`). Parts are fused into one
solid; there are no joints.
