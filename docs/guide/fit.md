# Fit and calibration

Three separate kinds of number (config file, see `ldraw2print config show`):

| Kind | Key | Meaning |
|------|-----|---------|
| Nominal | `nominal_diameter_ldu` | LDraw size of the mating part at 1:1, multiplied by the scale |
| Measured | `measured_diameter_mm` | calipers on YOUR figure; replaces nominal × scale |
| Fit / compensation | `[fit_presets]`, profile `fit`, `[printer]` | clearance you want + what your printer needs |

Target diameter:

- socket: mating + clearance + `hole_compensation_mm`
- pin:    mating − clearance − `pin_compensation_mm`

Presets are diametral clearances in mm (defaults loose 0.40, normal 0.20,
tight 0.05). `--fit clearance` uses the profile's `default_fit`;
`--clearance-mm X` overrides with an exact value.

LDraw circles are 16-gons: a socket's narrowest width is across the flats
(0.981 × the vertex diameter; 23.54 mm for a 24 mm stud socket at 5:1). The
fit targets across-flats for sockets and across-corners for pins, so the
numbers above are what you actually get. With `--smooth 96` the polygon is
nearly round (0.999): the polygon correction becomes negligible, and the
vertices move by roughly half the clearance you asked for.

## Calibrate

    ldraw2print calibrate --connector head-stud -o coupons.stl
    ldraw2print calibrate --connector head-stud --clearances -0.05,0,0.05,0.1

Coupon k has k notches. Print with the same printer, material and profile
you will use, test on the figure, then record the winner (same --scale and
--clearances as when you made the coupons):

    ldraw2print calibrate --connector head-stud --record 3 --printer "Bambu Lab H2S" --material PETG

This writes into the config in use (creating the user config if needed,
keeping its comments):

    [connectors.head-stud]
    default_fit = "tight"          # only when the coupon was a preset
    calibration = { date = "2026-10-25", printer = "Bambu Lab H2S", material = "PETG",
                    scale = 5.0, result = "coupon 3 (tight, +0.050 mm clearance, …) fit best",
                    clearance_mm = 0.05 }

`--fit clearance` then uses `clearance_mm` exactly, even for a custom
sweep value. Don't edit the TOML by hand unless you need to.

The `calibration` record is what makes a profile count as calibrated
(`ldraw2print profiles` shows [calibrated]; JSON `calibrated: true`).
Changing `default_fit` without a record shows as "your setting, no
calibration record". Converting at another scale than the record warns.

If the tightest (or loosest) coupon wins, the optimum may be beyond it:
print a finer sweep around it. If every coupon is equally off, change
`[printer] hole_compensation_mm` / `pin_compensation_mm` instead.

Calibration results hold for one printer + material + profile + scale.
