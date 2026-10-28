---
library: yes
config: default
---
# Record a calibration result, then use it

The only workflow where an agent edits the user's config. It must record a
proper calibration entry and not break the file.

## Prompt

I printed the head-stud calibration coupons from your tool on my Prusa MK4
in PLA at 5:1, and coupon 2 fit best. Please remember that for next time,
then make me a 5:1 police hat using that fit.

## Good outcome

- Finds the config (`config path`), sets head-stud `default_fit = "normal"`
  (coupon 2 = normal) and adds a `calibration` record (date, printer
  "Prusa MK4", material PLA, scale 5, result).
- Verifies with `profiles` that head-stud is now calibrated.
- Converts 3624 with `--connector head-stud --fit clearance` (or normal).
- Edits only the sandbox config the tool uses.

## Automatic checks

After the run, `ldraw2print --json profiles` in the sandbox reports
head-stud calibrated, default_fit normal, record scale 5, material contains
PLA, printer contains Prusa/MK4; an output file exists; the user's real
config is unchanged. A warning if the agent edited the TOML by hand instead
of using `calibrate --record 2`.
